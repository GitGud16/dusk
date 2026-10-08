//! The engine's front (docs/ARCHITECTURE.md, "Threading model" and "Data flow"). The UI
//! thread calls it and never waits for decoding: a video thread decodes and draws frames, a
//! mixer thread mixes sound, and both follow one playback clock. Results come back through
//! the callback the engine was made with.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use dusk_audio::PlaybackClock;
use dusk_core::time::STANDARD_RATES;
use dusk_core::time::{frame_at, frame_to_media};
use dusk_core::{Frame, MediaTime, Project, Rational};
use dusk_render::{Gpu, wgpu};

use crate::cache::DEFAULT_CAP;
use crate::export::{self, ExportJob};
use crate::sound::{self, SoundRequest};
use crate::video::{self, VideoRequest};
use crate::{EngineError, ExportEvent};

/// How long a decoder may sit unused before it is closed (docs/ARCHITECTURE.md, "Decoder
/// pool").
pub const DECODER_IDLE: Duration = Duration::from_secs(5);

/// What the engine reports. The callback runs on the engine's threads, so it should hand the
/// event to the UI thread and return.
#[derive(Debug)]
pub enum EngineEvent {
    /// The preview shows timeline frame `frame` now: drawn at the preview size, or black
    /// (`None`) where no video clip is visible.
    Frame {
        /// The timeline frame.
        frame: Frame,
        /// The picture, or `None` for black.
        texture: Option<wgpu::Texture>,
    },
    /// Playback reached the end of the sequence (or its start, playing backwards) and
    /// stopped there.
    Stopped {
        /// Where the playhead stopped.
        frame: Frame,
    },
    /// Something failed; the engine carries on where it can.
    Error(EngineError),
    /// An export moved on.
    Export(ExportEvent),
}

/// How an engine is set up.
#[derive(Clone, Debug)]
pub struct EngineOptions {
    /// Play sound on the default output device. Without sound, or without a device, the
    /// system clock drives playback.
    pub sound: bool,
    /// The byte cap of the frame cache.
    pub cache_cap: usize,
    /// How long an unused decoder stays open.
    pub decoder_idle: Duration,
}

impl Default for EngineOptions {
    fn default() -> EngineOptions {
        EngineOptions {
            sound: true,
            cache_cap: DEFAULT_CAP,
            decoder_idle: DECODER_IDLE,
        }
    }
}

/// Where playback is, shared by the front and both threads.
pub(crate) struct Transport {
    pub clock: PlaybackClock,
    /// Goes up with every play and stop, so a thread can tell its playback was replaced.
    pub generation: u64,
    /// The playback factor while playing (negative backwards); `None` while stopped.
    pub playing: Option<f64>,
    /// The sequence's frame rate and end.
    pub rate: Rational,
    pub end: Frame,
}

pub(crate) type SharedTransport = Arc<Mutex<Transport>>;

/// Locks the transport. A thread that panicked while holding it left nothing half-written
/// that matters more than stopping, so a poisoned lock is used as it is.
pub(crate) fn lock(transport: &Mutex<Transport>) -> MutexGuard<'_, Transport> {
    transport.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How the engine's threads report.
pub(crate) type Report = Arc<dyn Fn(EngineEvent) + Send + Sync>;

/// Plays, scrubs and exports the project it is given.
pub struct Engine {
    gpu: Gpu,
    video: Sender<VideoRequest>,
    sound: Sender<SoundRequest>,
    transport: SharedTransport,
    report: Report,
    open_decoders: Arc<AtomicUsize>,
    exporting: Arc<AtomicBool>,
    /// The last export started, so dropping the engine can stop it and clean up after it.
    export: Mutex<Option<(ExportJob, JoinHandle<()>)>>,
}

impl Engine {
    /// Starts the engine's threads on `gpu`. `on_event` is called on those threads.
    pub fn new(
        gpu: &Gpu,
        options: EngineOptions,
        on_event: impl Fn(EngineEvent) + Send + Sync + 'static,
    ) -> Result<Engine, EngineError> {
        let report: Report = Arc::new(on_event);
        let transport = Arc::new(Mutex::new(Transport {
            clock: PlaybackClock::stopped(0),
            generation: 0,
            playing: None,
            // 30 fps, the default sequence rate, until a project arrives.
            rate: STANDARD_RATES[4],
            end: Frame(0),
        }));
        let open_decoders = Arc::new(AtomicUsize::new(0));
        let exporting = Arc::new(AtomicBool::new(false));
        let video = video::spawn(
            gpu,
            &options,
            Arc::clone(&transport),
            Arc::clone(&report),
            Arc::clone(&open_decoders),
            Arc::clone(&exporting),
        )?;
        let sound = sound::spawn(&options, Arc::clone(&transport), Arc::clone(&report))?;
        Ok(Engine {
            gpu: gpu.clone(),
            video,
            sound,
            transport,
            report,
            open_decoders,
            exporting,
            export: Mutex::new(None),
        })
    }

    /// Uses `project` from now on. The preview is not redrawn until the next request.
    pub fn set_project(&self, project: Arc<Project>) {
        {
            let mut transport = lock(&self.transport);
            transport.rate = project.sequence().frame_rate();
            transport.end = project.sequence().end();
        }
        // Sending fails only when a thread has ended, which happens only as the engine drops.
        let _ = self.video.send(VideoRequest::Project(Arc::clone(&project)));
        let _ = self.sound.send(SoundRequest::Project(project));
    }

    /// Draws the preview at `size` (width, height) in pixels from now on, and redraws it.
    pub fn set_preview_size(&self, size: (u32, u32)) {
        let _ = self.video.send(VideoRequest::Size(size));
    }

    /// Stops playing, if it was, and shows exactly timeline frame `frame`.
    pub fn show(&self, frame: Frame) {
        self.stop();
        let _ = self.video.send(VideoRequest::Show(frame));
    }

    /// Stops playing, if it was, and shows timeline frame `frame` while the playhead is
    /// dragged: the nearest cached frame at once, the exact one at most every 16 ms
    /// (docs/ARCHITECTURE.md, "Scrubbing"). End a drag with [`show`](Self::show).
    pub fn scrub(&self, frame: Frame) {
        self.stop();
        let _ = self.video.send(VideoRequest::Scrub(frame));
    }

    /// Plays from `from` at `factor` times normal speed, backwards when negative. Sound plays
    /// from 0.25x to 4x forwards; outside that, and backwards, playback is silent. Nothing
    /// plays while an export runs.
    pub fn play(&self, from: Frame, factor: f64) {
        if self.is_exporting() {
            return;
        }
        let (generation, from) = {
            let mut transport = lock(&self.transport);
            transport.generation += 1;
            let at = frame_to_media(from, transport.rate);
            transport.clock = PlaybackClock::stopped(at.0);
            transport.playing = Some(factor);
            (transport.generation, at)
        };
        let _ = self.video.send(VideoRequest::Play { generation });
        let _ = self.sound.send(SoundRequest::Play {
            generation,
            from,
            factor,
        });
    }

    /// Stops playing and returns the frame it stopped at, which the preview then shows;
    /// `None` when it was not playing.
    pub fn pause(&self) -> Option<Frame> {
        let frame = self.stop()?;
        let _ = self.video.send(VideoRequest::Show(frame));
        Some(frame)
    }

    /// The playback factor while playing, `None` while stopped.
    pub fn playing(&self) -> Option<f64> {
        lock(&self.transport).playing
    }

    /// How many video decoders are open; each holds memory until it is closed after
    /// [`EngineOptions::decoder_idle`] unused.
    pub fn open_decoders(&self) -> usize {
        self.open_decoders.load(Ordering::Relaxed)
    }

    /// Exports `project` to an MP4 file at `path` on its own thread; progress and the outcome
    /// arrive as [`EngineEvent::Export`]. Playback stops, and until the export ends the
    /// preview shows cached frames only (docs/ARCHITECTURE.md, "Export").
    pub fn export(&self, project: Arc<Project>, path: PathBuf) -> Result<ExportJob, EngineError> {
        let mut export = self.export.lock().unwrap_or_else(PoisonError::into_inner);
        if self.is_exporting() {
            return Err(EngineError::ExportRunning);
        }
        // The previous export has ended; its thread is only reporting or gone.
        if let Some((_, thread)) = export.take() {
            let _ = thread.join();
        }
        self.stop();
        let (job, thread) = export::spawn(
            &self.gpu,
            project,
            path,
            Arc::clone(&self.exporting),
            Arc::clone(&self.report),
        )?;
        *export = Some((job.clone(), thread));
        Ok(job)
    }

    /// Whether an export is running.
    pub fn is_exporting(&self) -> bool {
        self.exporting.load(Ordering::Relaxed)
    }

    /// Stops the clock and the sound, if playing, and returns the frame it stopped at.
    fn stop(&self) -> Option<Frame> {
        let frame = {
            let mut transport = lock(&self.transport);
            transport.playing?;
            transport.generation += 1;
            transport.playing = None;
            let at = transport.clock.stop(Instant::now());
            let last = (transport.end - Frame(1)).max(Frame(0));
            frame_at(MediaTime(at), transport.rate).clamp(Frame(0), last)
        };
        let _ = self.sound.send(SoundRequest::Stop);
        Some(frame)
    }
}

impl Drop for Engine {
    /// Cancels a running export and waits for it to remove what it wrote, so quitting never
    /// leaves a `.part` file behind.
    fn drop(&mut self) {
        let export = self
            .export
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some((job, thread)) = export {
            job.cancel();
            let _ = thread.join();
        }
    }
}
