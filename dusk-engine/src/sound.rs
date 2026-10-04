//! The audio thread (docs/ARCHITECTURE.md, "Playback"): while sound plays it keeps the output
//! device's buffer full from the mixer and starts the playback clock on the device, so video
//! follows audio. When there is nothing to hear (silent speeds, playing backwards, no device)
//! it starts the clock on the system clock instead.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use dusk_audio::{AudioOutput, ClockSource};
use dusk_core::{MediaTime, Project};

use crate::EngineError;
use crate::engine::{EngineEvent, EngineOptions, Report, SharedTransport, lock};
use crate::mixer::Mixer;

/// What the front asks of the audio thread.
pub(crate) enum SoundRequest {
    Project(Arc<Project>),
    Play {
        generation: u64,
        from: MediaTime,
        factor: f64,
    },
    Stop,
}

/// Frames mixed at a time: 10 ms at 48 kHz.
const BLOCK: usize = 480;
/// While sound plays, the buffer is topped up at least this often; it holds 200 ms.
const FEED_INTERVAL: Duration = Duration::from_millis(5);
/// Sound plays at these playback factors, forwards only (docs/ARCHITECTURE.md, "Playback").
const AUDIBLE: std::ops::RangeInclusive<f64> = 0.25..=4.0;

/// Starts the audio thread.
pub(crate) fn spawn(
    options: &EngineOptions,
    transport: SharedTransport,
    report: Report,
) -> Result<Sender<SoundRequest>, EngineError> {
    let (sender, inbox) = crossbeam_channel::unbounded();
    let sound = options.sound;
    std::thread::Builder::new()
        .name("dusk audio".to_owned())
        .spawn(move || {
            SoundThread {
                sound,
                transport,
                report,
                project: None,
                output: None,
                no_device: false,
                playing: None,
            }
            .run(&inbox)
        })
        .map_err(EngineError::Thread)?;
    Ok(sender)
}

struct SoundThread {
    /// Whether to use the output device at all.
    sound: bool,
    transport: SharedTransport,
    report: Report,
    project: Option<Arc<Project>>,
    /// Opened the first time sound plays, then kept.
    output: Option<AudioOutput>,
    /// The machine has no output device, or it failed to open.
    no_device: bool,
    playing: Option<Playing>,
}

/// Sound being played.
struct Playing {
    generation: u64,
    factor: f64,
    mixer: Mixer,
    block: Vec<f32>,
}

impl SoundThread {
    fn run(mut self, inbox: &Receiver<SoundRequest>) {
        loop {
            let first = if self.playing.is_some() {
                match inbox.recv_timeout(FEED_INTERVAL) {
                    Ok(request) => Some(request),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match inbox.recv() {
                    Ok(request) => Some(request),
                    Err(_) => return,
                }
            };
            for request in first.into_iter().chain(inbox.try_iter()) {
                match request {
                    SoundRequest::Project(project) => self.project = Some(project),
                    SoundRequest::Stop => self.stop(),
                    SoundRequest::Play {
                        generation,
                        from,
                        factor,
                    } => self.play(generation, from, factor),
                }
            }
            self.feed();
        }
    }

    fn play(&mut self, generation: u64, from: MediaTime, factor: f64) {
        self.stop();
        if self.sound && AUDIBLE.contains(&factor) {
            self.open_device();
        }
        let output = self.output.as_mut().filter(|_| AUDIBLE.contains(&factor));
        let (Some(output), Some(project)) = (output, self.project.clone()) else {
            return self.start_system_clock(generation, from, factor);
        };
        let mut playing = Playing {
            generation,
            factor,
            mixer: Mixer::new(project, output.rate(), from, factor),
            block: vec![0.0; 2 * BLOCK],
        };
        // A full buffer before the device starts, so it never starts on silence.
        if let Err(error) = top_up(output, &mut playing) {
            (self.report)(EngineEvent::Error(error));
            return self.start_system_clock(generation, from, factor);
        }
        {
            let mut transport = lock(&self.transport);
            if transport.generation != generation {
                return;
            }
            let device = ClockSource::Audio(output.frame_counter());
            transport
                .clock
                .start(from.0, factor, device, Instant::now());
        }
        if let Err(error) = output.start() {
            (self.report)(EngineEvent::Error(error.into()));
            return self.switch_to_system_clock(generation, factor);
        }
        self.playing = Some(playing);
    }

    /// Keeps the device's buffer full while the playback this thread started is current.
    fn feed(&mut self) {
        let (Some(playing), Some(output)) = (self.playing.as_mut(), self.output.as_mut()) else {
            return;
        };
        if lock(&self.transport).generation != playing.generation {
            return self.stop();
        }
        if let Err(error) = top_up(output, playing) {
            let (generation, factor) = (playing.generation, playing.factor);
            (self.report)(EngineEvent::Error(error));
            self.stop();
            self.switch_to_system_clock(generation, factor);
        }
    }

    fn stop(&mut self) {
        let Some(_) = self.playing.take() else {
            return;
        };
        if let Some(Err(error)) = self.output.as_ref().map(AudioOutput::stop) {
            (self.report)(EngineEvent::Error(error.into()));
        }
    }

    fn open_device(&mut self) {
        if self.output.is_some() || self.no_device {
            return;
        }
        match AudioOutput::open() {
            Ok(Some(output)) => self.output = Some(output),
            Ok(None) => self.no_device = true,
            Err(error) => {
                self.no_device = true;
                (self.report)(EngineEvent::Error(error.into()));
            }
        }
    }

    /// Runs playback `generation` silently on the system clock from `from`.
    fn start_system_clock(&self, generation: u64, from: MediaTime, factor: f64) {
        let mut transport = lock(&self.transport);
        if transport.generation == generation {
            transport
                .clock
                .start(from.0, factor, ClockSource::System, Instant::now());
        }
    }

    /// Carries playback `generation` on silently from where the device clock got to.
    fn switch_to_system_clock(&self, generation: u64, factor: f64) {
        let now = Instant::now();
        let mut transport = lock(&self.transport);
        if transport.generation == generation {
            let at = transport.clock.time(now);
            transport.clock.start(at, factor, ClockSource::System, now);
        }
    }
}

/// Mixes blocks into the device's buffer until it is full.
fn top_up(output: &mut AudioOutput, playing: &mut Playing) -> Result<(), EngineError> {
    while output.space() >= BLOCK {
        playing.mixer.fill(&mut playing.block)?;
        output.write(&playing.block);
    }
    Ok(())
}
