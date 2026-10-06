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
use dusk_core::{Frame, MediaId, MediaInfo, MediaTime, Project, Rational};
use dusk_render::{Gpu, wgpu};

use crate::cache::DEFAULT_CAP;
use crate::export::{self, ExportJob};
use crate::settings::{ExportFormat, ExportSettings, has_sound};
use crate::sound::{self, SoundRequest};
use crate::thumbnail::{self, Thumbnail, ThumbnailJob};
use crate::video::{self, VideoRequest};
use crate::{EngineError, ExportEvent};

/// How long a decoder may sit unused before it is closed (docs/ARCHITECTURE.md, "Decoder
/// pool").
pub const DECODER_IDLE: Duration = Duration::from_secs(5);

/// Which of the two previews a request or an event is about (docs/ARCHITECTURE.md,
/// "Pop-out clip editor"). Each shows a project of its own, through one frame cache and one
/// decoder pool, and only one plays at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preview {
    /// The main window's: the whole sequence.
    Main,
    /// The clip editor's: one clip and its linked partner, as drafted.
    ClipEditor,
}

impl Preview {
    /// Both previews, in the order of [`Preview::index`].
    pub const ALL: [Preview; 2] = [Preview::Main, Preview::ClipEditor];

    /// The preview's place in [`Preview::ALL`], for keeping something per preview.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// What the engine reports. The callback runs on the engine's threads, so it should hand the
/// event to the UI thread and return.
#[derive(Debug)]
pub enum EngineEvent {
    /// `preview` shows timeline frame `frame` now: the sequence's frame as large as fits in
    /// the preview, black where no video clip is visible.
    Frame {
        /// The preview drawn.
        preview: Preview,
        /// The timeline frame.
        frame: Frame,
        /// The frame drawn, or `None` when there is nothing to show.
        texture: Option<wgpu::Texture>,
    },
    /// Playback in `preview` reached the end of its sequence (or its start, playing
    /// backwards) and stopped there.
    Stopped {
        /// The preview that played.
        preview: Preview,
        /// Where the playhead stopped.
        frame: Frame,
    },
    /// Something failed; the engine carries on where it can.
    Error(EngineError),
    /// An export moved on.
    Export(ExportEvent),
    /// The thumbnail of a media file is ready.
    Thumbnail {
        media: MediaId,
        thumbnail: Thumbnail,
    },
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
    /// The preview playing, or that played last; the clock counts its sequence's time.
    pub preview: Preview,
    /// Each preview's sequence frame rate and end, by [`Preview::index`].
    pub timing: [(Rational, Frame); 2],
}

impl Transport {
    /// The frame rate of the playing preview's sequence.
    pub fn rate(&self) -> Rational {
        self.timing[self.preview.index()].0
    }

    /// The end of the playing preview's sequence.
    pub fn end(&self) -> Frame {
        self.timing[self.preview.index()].1
    }
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
    thumbnails: Sender<ThumbnailJob>,
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
            preview: Preview::Main,
            // 30 fps, the default sequence rate, until a project arrives.
            timing: [(STANDARD_RATES[4], Frame(0)); 2],
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
        let thumbnails = thumbnail::spawn(
            Arc::clone(&transport),
            Arc::clone(&exporting),
            Arc::clone(&report),
        )?;
        Ok(Engine {
            gpu: gpu.clone(),
            video,
            sound,
            transport,
            report,
            open_decoders,
            exporting,
            thumbnails,
            export: Mutex::new(None),
        })
    }

    /// Makes the thumbnail of media `media`, the file at `path` described by `info`, on the
    /// thumbnail thread, which reports it as [`EngineEvent::Thumbnail`]: one at a time, in the
    /// order asked, while nothing plays or exports. Sound alone gets none.
    pub fn make_thumbnail(&self, media: MediaId, path: PathBuf, info: MediaInfo) {
        let _ = self.thumbnails.send(ThumbnailJob { media, path, info });
    }

    /// Shows `project` in `preview` from now on. The preview is not redrawn until the next
    /// request.
    pub fn set_project(&self, preview: Preview, project: Arc<Project>) {
        {
            let mut transport = lock(&self.transport);
            let sequence = project.sequence();
            transport.timing[preview.index()] = (sequence.frame_rate(), sequence.end());
        }
        // Sending fails only when a thread has ended, which happens only as the engine drops.
        let _ = self
            .video
            .send(VideoRequest::Project(preview, Arc::clone(&project)));
        let _ = self.sound.send(SoundRequest::Project(preview, project));
    }

    /// Draws `preview` at `size` (width, height) in pixels from now on, and redraws it.
    pub fn set_preview_size(&self, preview: Preview, size: (u32, u32)) {
        let _ = self.video.send(VideoRequest::Size(preview, size));
    }

    /// Lets go of what `preview` shows and draws with, once its window closed; playback in it
    /// stops. Setting its project and size again brings it back.
    pub fn close_preview(&self, preview: Preview) {
        if self
            .playing()
            .is_some_and(|(playing, _)| playing == preview)
        {
            self.stop();
        }
        let _ = self.video.send(VideoRequest::Close(preview));
        let _ = self.sound.send(SoundRequest::Close(preview));
    }

    /// Stops playing, if anything was, and shows exactly timeline frame `frame` in `preview`.
    pub fn show(&self, preview: Preview, frame: Frame) {
        self.stop();
        let _ = self.video.send(VideoRequest::Show(preview, frame));
    }

    /// Stops playing, if anything was, and shows timeline frame `frame` in `preview` while
    /// its playhead is dragged: the nearest cached frame at once, the exact one at most every
    /// 16 ms (docs/ARCHITECTURE.md, "Scrubbing"). End a drag with [`show`](Self::show).
    pub fn scrub(&self, preview: Preview, frame: Frame) {
        self.stop();
        let _ = self.video.send(VideoRequest::Scrub(preview, frame));
    }

    /// Plays `preview` from `from` at `factor` times normal speed, backwards when negative,
    /// stopping playback in the other preview: one plays at a time. Sound plays from 0.25x to
    /// 4x forwards and from 0.25x to 2x backwards; outside that, playback is silent. Nothing
    /// plays while an export runs.
    pub fn play(&self, preview: Preview, from: Frame, factor: f64) {
        if self.is_exporting() {
            return;
        }
        let (generation, from) = {
            let mut transport = lock(&self.transport);
            transport.generation += 1;
            transport.preview = preview;
            let at = frame_to_media(from, transport.rate());
            transport.clock = PlaybackClock::stopped(at.0);
            transport.playing = Some(factor);
            (transport.generation, at)
        };
        let _ = self.video.send(VideoRequest::Play {
            preview,
            generation,
        });
        let _ = self.sound.send(SoundRequest::Play {
            preview,
            generation,
            from,
            factor,
        });
    }

    /// Stops playing and returns the preview that played and the frame it stopped at, which
    /// that preview then shows; `None` when nothing was playing.
    pub fn pause(&self) -> Option<(Preview, Frame)> {
        let (preview, frame) = self.stop()?;
        let _ = self.video.send(VideoRequest::Show(preview, frame));
        Some((preview, frame))
    }

    /// The preview playing and its playback factor, `None` while stopped.
    pub fn playing(&self) -> Option<(Preview, f64)> {
        let transport = lock(&self.transport);
        transport.playing.map(|factor| (transport.preview, factor))
    }

    /// How many video decoders are open; each holds memory until it is closed after
    /// [`EngineOptions::decoder_idle`] unused.
    pub fn open_decoders(&self) -> usize {
        self.open_decoders.load(Ordering::Relaxed)
    }

    /// Exports `project` to an MP4 file at `path` on its own thread; progress and the outcome
    /// arrive as [`EngineEvent::Export`]. Playback stops, and until the export ends the
    /// preview shows cached frames only (docs/ARCHITECTURE.md, "Export").
    pub fn export(
        &self,
        project: Arc<Project>,
        path: PathBuf,
        settings: ExportSettings,
    ) -> Result<ExportJob, EngineError> {
        if matches!(settings.format, ExportFormat::Sound(_)) && !has_sound(&project) {
            return Err(EngineError::NoSound);
        }
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
            settings,
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

    /// Stops the clock and the sound, if playing, and returns the preview that played and the
    /// frame it stopped at.
    fn stop(&self) -> Option<(Preview, Frame)> {
        let stopped = {
            let mut transport = lock(&self.transport);
            transport.playing?;
            transport.generation += 1;
            transport.playing = None;
            let at = transport.clock.stop(Instant::now());
            let last = (transport.end() - Frame(1)).max(Frame(0));
            let frame = frame_at(MediaTime(at), transport.rate()).clamp(Frame(0), last);
            (transport.preview, frame)
        };
        let _ = self.sound.send(SoundRequest::Stop);
        Some(stopped)
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
