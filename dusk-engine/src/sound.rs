//! The mixer thread (docs/ARCHITECTURE.md, "Playback"): while sound plays it keeps the output
//! device's buffer full from the mixer, forwards or backwards, and starts the playback clock
//! on the device, so video follows audio. When there is nothing to hear (a silent speed, no
//! device) it starts the clock on the system clock instead.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use dusk_audio::{AudioOutput, ClockSource};
use dusk_core::{MediaTime, Project};
use dusk_media::MediaError;

use crate::EngineError;
use crate::engine::{EngineEvent, EngineOptions, Preview, Report, SharedTransport, lock};
use crate::mixer::{Mixer, ReverseMixer};

/// What the front asks of the mixer thread.
pub(crate) enum SoundRequest {
    Project(Preview, Arc<Project>),
    Play {
        preview: Preview,
        generation: u64,
        from: MediaTime,
        factor: f64,
    },
    Stop,
    /// The preview's window closed.
    Close(Preview),
}

/// Frames mixed at a time: 10 ms at 48 kHz.
const BLOCK: usize = 480;
/// While sound plays, the buffer is topped up at least this often; it holds 200 ms.
const FEED_INTERVAL: Duration = Duration::from_millis(5);
/// Whether sound plays at playback factor `factor` (docs/ARCHITECTURE.md, "Playback"): from
/// 0.25x to 4x forwards and from 0.25x to 2x backwards.
fn audible(factor: f64) -> bool {
    (0.25..=4.0).contains(&factor) || (-2.0..=-0.25).contains(&factor)
}

/// Starts the mixer thread.
pub(crate) fn spawn(
    options: &EngineOptions,
    transport: SharedTransport,
    report: Report,
) -> Result<Sender<SoundRequest>, EngineError> {
    let (sender, inbox) = crossbeam_channel::unbounded();
    let sound = options.sound;
    std::thread::Builder::new()
        .name("dusk mixer".to_owned())
        .spawn(move || {
            SoundThread {
                sound,
                transport,
                report,
                projects: [None, None],
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
    /// Each preview's project, by [`Preview::index`].
    projects: [Option<Arc<Project>>; 2],
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
    mixer: Direction,
    block: Vec<f32>,
}

/// The sequence mixed one way or the other.
enum Direction {
    Forwards(Mixer),
    Backwards(ReverseMixer),
}

impl Direction {
    /// Mixes `project` from `from` at `rate` frames per second, backwards when `factor` is
    /// negative.
    fn new(project: Arc<Project>, rate: u32, from: MediaTime, factor: f64) -> Direction {
        if factor < 0.0 {
            Direction::Backwards(ReverseMixer::new(project, rate, from, -factor))
        } else {
            Direction::Forwards(Mixer::new(project, rate, from, factor))
        }
    }

    fn fill(&mut self, out: &mut [f32]) -> Result<(), MediaError> {
        match self {
            Direction::Forwards(mixer) => mixer.fill(out),
            Direction::Backwards(mixer) => mixer.fill(out),
        }
    }
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
                    SoundRequest::Project(preview, project) => {
                        self.projects[preview.index()] = Some(project);
                    }
                    SoundRequest::Stop => self.stop(),
                    SoundRequest::Close(preview) => self.projects[preview.index()] = None,
                    SoundRequest::Play {
                        preview,
                        generation,
                        from,
                        factor,
                    } => self.play(preview, generation, from, factor),
                }
            }
            self.feed();
        }
    }

    fn play(&mut self, preview: Preview, generation: u64, from: MediaTime, factor: f64) {
        self.stop();
        if self.sound && audible(factor) {
            self.open_device();
        }
        let output = self.output.as_mut().filter(|_| audible(factor));
        let project = self.projects[preview.index()].clone();
        let (Some(output), Some(project)) = (output, project) else {
            return self.start_system_clock(generation, from, factor);
        };
        let mut playing = Playing {
            generation,
            factor,
            mixer: Direction::new(project, output.rate(), from, factor),
            block: vec![0.0; 2 * BLOCK],
        };
        // A full buffer before the device starts, so it never starts on silence.
        if let Err(error) = top_up(output, &mut playing) {
            (self.report)(EngineEvent::Error(error.or_gone(Path::is_file)));
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
            // A drive that went away under a clip's sound fails as FFmpeg failing to read it.
            (self.report)(EngineEvent::Error(error.or_gone(Path::is_file)));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_plays_from_a_quarter_speed_to_4x_forwards_and_2x_backwards() {
        for factor in [0.25, 1.0, 4.0, -0.25, -1.0, -2.0] {
            assert!(audible(factor), "silent at {factor}x");
        }
        for factor in [0.1, 8.0, 32.0, -0.1, -4.0, -32.0] {
            assert!(!audible(factor), "audible at {factor}x");
        }
    }
}
