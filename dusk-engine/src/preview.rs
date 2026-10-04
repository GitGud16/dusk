//! The preview worker: decodes and composites one file's frames off the UI thread, always
//! serving the newest request first.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use dusk_core::MediaTime;
use dusk_media::{Acceleration, MediaError, VideoDecoder};
use dusk_render::wgpu;
use dusk_render::{Compositor, Gpu, RenderError};

/// What the preview worker reports.
#[derive(Debug)]
pub enum PreviewEvent {
    /// A frame is ready to show.
    Frame {
        /// The frame's timestamp in the source.
        time: MediaTime,
        /// The frame, drawn at the requested size on the shared device.
        texture: wgpu::Texture,
    },
    /// Nothing is shown at the requested time, which is before the first frame.
    Nothing {
        /// The requested time.
        time: MediaTime,
    },
    /// The file could not be opened, decoded or drawn; the worker carries on if it can.
    Error(PreviewError),
}

/// Why the preview could not show a frame.
#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    /// Opening or decoding the file failed.
    #[error(transparent)]
    Media(#[from] MediaError),
    /// Drawing the frame failed.
    #[error(transparent)]
    Render(#[from] RenderError),
    /// The worker thread could not start.
    #[error("Dusk could not start a worker thread ({0}); close some programs and try again")]
    Thread(std::io::Error),
}

/// How long a decoder may sit unused before it is closed (docs/ARCHITECTURE.md, "Decoder
/// pool"); a hardware decoder holds tens of megabytes of surfaces while open.
pub const DECODER_IDLE: Duration = Duration::from_secs(5);

/// Decodes and composites one file's frames on a worker thread. The worker ends when this is
/// dropped.
pub struct Preview {
    requests: Sender<Request>,
    decoding: Arc<AtomicBool>,
}

/// One frame the UI asked for.
struct Request {
    time: MediaTime,
    size: (u32, u32),
}

impl Preview {
    /// Starts a worker for the video in `path` that closes its decoder after `idle` without
    /// requests ([`DECODER_IDLE`] in the app) and reopens it when asked again. `on_event` runs
    /// on the worker for every result, so it should hand the result to the UI thread and
    /// return.
    pub fn open(
        gpu: &Gpu,
        path: PathBuf,
        idle: Duration,
        on_event: impl Fn(PreviewEvent) + Send + 'static,
    ) -> Result<Preview, PreviewError> {
        let (requests, inbox) = crossbeam_channel::unbounded();
        let decoding = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            gpu: gpu.clone(),
            path,
            idle,
            decoding: Arc::clone(&decoding),
        };
        std::thread::Builder::new()
            .name("dusk preview".to_owned())
            .spawn(move || worker.run(&inbox, &on_event))
            .map_err(PreviewError::Thread)?;
        Ok(Preview { requests, decoding })
    }

    /// Whether the worker holds an open decoder right now.
    pub fn is_decoding(&self) -> bool {
        self.decoding.load(Ordering::Relaxed)
    }

    /// Asks for the frame at `time`, drawn at `size` (width, height) in pixels. Requests the
    /// worker has not started yet are dropped in favor of the newest one.
    pub fn show(&self, time: MediaTime, size: (u32, u32)) {
        // Sending fails only once the worker has stopped after reporting why.
        let _ = self.requests.send(Request { time, size });
    }
}

/// What the worker thread owns.
struct Worker {
    gpu: Gpu,
    path: PathBuf,
    idle: Duration,
    decoding: Arc<AtomicBool>,
}

impl Worker {
    /// Opens the file, then answers requests until the `Preview` is dropped.
    fn run(&self, inbox: &Receiver<Request>, on_event: &dyn Fn(PreviewEvent)) {
        // Opened at once, so a file that cannot be read is reported before any request.
        let mut decoder = match self.open_decoder() {
            Ok(decoder) => Some(decoder),
            Err(error) => return on_event(PreviewEvent::Error(error.into())),
        };
        let compositor = Compositor::new(&self.gpu);
        loop {
            let first = match inbox.recv_timeout(self.idle) {
                Ok(request) => request,
                Err(RecvTimeoutError::Timeout) => {
                    decoder = None;
                    self.decoding.store(false, Ordering::Relaxed);
                    match inbox.recv() {
                        Ok(request) => request,
                        Err(_) => return,
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            };
            // Scrubbing sends a stream of requests; only the newest one is worth drawing.
            let request = inbox.try_iter().last().unwrap_or(first);
            if decoder.is_none() {
                match self.open_decoder() {
                    Ok(reopened) => decoder = Some(reopened),
                    Err(error) => {
                        on_event(PreviewEvent::Error(error.into()));
                        continue;
                    }
                }
            }
            let Some(active) = decoder.as_mut() else {
                continue;
            };
            on_event(answer(active, &compositor, &request));
        }
    }

    fn open_decoder(&self) -> Result<VideoDecoder, MediaError> {
        // Software decoding (docs/ARCHITECTURE.md, "Decoder pool"): measured on 1080p and 4K
        // clips it is as fast as the hardware decoder once frames are copied back, and holds
        // a fraction of its memory.
        let decoder = VideoDecoder::open(&self.path, Acceleration::Software)?;
        self.decoding.store(true, Ordering::Relaxed);
        Ok(decoder)
    }
}

/// Decodes and draws the frame `request` asks for.
fn answer(decoder: &mut VideoDecoder, compositor: &Compositor, request: &Request) -> PreviewEvent {
    match decoder.frame_at(request.time) {
        Ok(Some(frame)) => match compositor.render(&frame.picture, request.size) {
            Ok(texture) => PreviewEvent::Frame {
                time: frame.time,
                texture,
            },
            Err(error) => PreviewEvent::Error(error.into()),
        },
        Ok(None) => PreviewEvent::Nothing { time: request.time },
        Err(error) => PreviewEvent::Error(error.into()),
    }
}
