//! The video thread (docs/ARCHITECTURE.md, "Data flow" and "Decoder pool"): it finds the clip
//! visible at a timeline frame, takes that clip's frame from the cache or decodes it, draws it
//! at the preview size and reports it. While playing it follows the playback clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use dusk_audio::PlaybackClock;
use dusk_core::time::{frame_at, frame_to_media};
use dusk_core::{Frame, MediaId, MediaTime, Picture, Project};
use dusk_media::{Acceleration, Following, VideoDecoder};
use dusk_render::{Compositor, Gpu};

use crate::EngineError;
use crate::cache::FrameCache;
use crate::engine::{EngineEvent, EngineOptions, Report, SharedTransport, lock};

/// What the front asks of the video thread.
pub(crate) enum VideoRequest {
    Project(Arc<Project>),
    Size((u32, u32)),
    Show(Frame),
    Scrub(Frame),
    Play { generation: u64 },
}

/// While scrubbing, exact frames are decoded at most this often.
const SCRUB_INTERVAL: Duration = Duration::from_millis(16);
/// While playing, the clock is read at least this often.
const PLAYBACK_POLL: Duration = Duration::from_millis(10);
/// Sources with more pixels than this get a decoder to themselves (docs/ARCHITECTURE.md,
/// "Decoder pool": the 1080p class ends at 2.1 Mpx).
const LARGE_FRAME: u64 = 2_100_000;

/// Starts the video thread.
pub(crate) fn spawn(
    gpu: &Gpu,
    options: &EngineOptions,
    transport: SharedTransport,
    report: Report,
    open_decoders: Arc<AtomicUsize>,
    exporting: Arc<AtomicBool>,
) -> Result<Sender<VideoRequest>, EngineError> {
    let (sender, inbox) = crossbeam_channel::unbounded();
    let gpu = gpu.clone();
    let (cap, idle) = (options.cache_cap, options.decoder_idle);
    std::thread::Builder::new()
        .name("dusk video".to_owned())
        .spawn(move || {
            VideoThread {
                compositor: Compositor::new(&gpu),
                transport,
                report,
                open_decoders,
                exporting,
                idle,
                project: None,
                size: (0, 0),
                cache: FrameCache::new(cap),
                decoders: Vec::new(),
                playing: None,
                target: None,
                shown_exactly: None,
                pending_scrub: None,
                last_exact: None,
            }
            .run(&inbox)
        })
        .map_err(EngineError::Thread)?;
    Ok(sender)
}

/// What the preview shows at a frame.
enum Shown {
    Picture(Arc<Picture>),
    Black,
    /// Not known without decoding.
    Unknown,
}

/// How far to go for a frame that is not cached.
#[derive(Clone, Copy)]
enum Fetch {
    /// Not at all.
    Cached,
    /// Take the cached frame of the same clip nearest in time.
    Nearest,
    /// Decode it.
    Decode,
    /// Decode its group of pictures from the keyframe and cache every frame of it, for
    /// playing backwards.
    Gop,
}

struct OpenDecoder {
    media: MediaId,
    decoder: VideoDecoder,
    large: bool,
    last_used: Instant,
}

struct VideoThread {
    compositor: Compositor,
    transport: SharedTransport,
    report: Report,
    open_decoders: Arc<AtomicUsize>,
    /// While an export runs, the preview shows cached frames only.
    exporting: Arc<AtomicBool>,
    idle: Duration,
    project: Option<Arc<Project>>,
    size: (u32, u32),
    cache: FrameCache,
    decoders: Vec<OpenDecoder>,
    /// The playback this thread follows, by generation.
    playing: Option<u64>,
    /// The frame the preview should show.
    target: Option<Frame>,
    /// The frame the preview shows exactly, if it does.
    shown_exactly: Option<Frame>,
    /// A scrub position shown only approximately so far.
    pending_scrub: Option<Frame>,
    last_exact: Option<Instant>,
}

impl VideoThread {
    fn run(mut self, inbox: &Receiver<VideoRequest>) {
        loop {
            let first = match self.wait() {
                Some(wait) => match inbox.recv_timeout(wait) {
                    Ok(request) => Some(request),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match inbox.recv() {
                    Ok(request) => Some(request),
                    Err(_) => return,
                },
            };
            // Requests pile up while a frame is decoded; only the newest frame request counts.
            let mut wanted = None;
            let mut redraw = false;
            for request in first.into_iter().chain(inbox.try_iter()) {
                match request {
                    VideoRequest::Project(project) => {
                        self.project = Some(project);
                        // An edit can change what any frame shows.
                        self.shown_exactly = None;
                    }
                    VideoRequest::Size(size) => {
                        self.size = size;
                        redraw = true;
                    }
                    VideoRequest::Show(frame) => {
                        self.playing = None;
                        wanted = Some((frame, true));
                    }
                    VideoRequest::Scrub(frame) => {
                        self.playing = None;
                        wanted = Some((frame, false));
                    }
                    VideoRequest::Play { generation } => {
                        self.playing = Some(generation);
                        self.pending_scrub = None;
                        wanted = None;
                    }
                }
            }
            if let Some(generation) = self.playing {
                self.follow_clock(generation);
            } else if let Some((frame, exact)) = wanted {
                if exact {
                    self.show_exact(frame);
                } else {
                    self.scrub(frame);
                }
            } else if let Some(frame) = self.pending_scrub.filter(|_| self.scrub_due()) {
                self.show_exact(frame);
            } else if let Some(frame) = self.target.filter(|_| redraw) {
                self.show_exact(frame);
            }
            let now = Instant::now();
            let idle = self.idle;
            self.decoders
                .retain(|open| now.duration_since(open.last_used) < idle);
            self.open_decoders
                .store(self.decoders.len(), Ordering::Relaxed);
        }
    }

    /// How long to wait for a request before there is work of its own; `None` for as long as
    /// it takes.
    fn wait(&self) -> Option<Duration> {
        if self.playing.is_some() {
            return Some(self.until_next_frame().min(PLAYBACK_POLL));
        }
        let now = Instant::now();
        let scrub = self.pending_scrub.map(|_| {
            self.last_exact.map_or(Duration::ZERO, |last| {
                (last + SCRUB_INTERVAL).saturating_duration_since(now)
            })
        });
        let idle = self
            .decoders
            .iter()
            .map(|open| (open.last_used + self.idle).saturating_duration_since(now))
            .min();
        match (scrub, idle) {
            (Some(scrub), Some(idle)) => Some(scrub.min(idle)),
            (scrub, idle) => scrub.or(idle),
        }
    }

    /// How long until the playback clock reaches the next frame; zero when the frame on
    /// screen is not the one the clock is at.
    fn until_next_frame(&self) -> Duration {
        let transport = lock(&self.transport);
        let (Some(factor), true) = (transport.playing, transport.clock.is_running()) else {
            // Waiting for the sound to start the clock.
            return Duration::from_millis(1);
        };
        let rate = transport.rate;
        let time = transport.clock.time(Instant::now());
        let frame = frame_at(MediaTime(time), rate);
        if self.shown_exactly != Some(frame) {
            return Duration::ZERO;
        }
        // The next frame starts where this one ends, or, backwards, just before this one.
        let next = if factor > 0.0 {
            frame_to_media(frame + Frame(1), rate).0
        } else {
            frame_to_media(frame, rate).0 - 1
        };
        let micros = (next - time).unsigned_abs() as f64 / factor.abs().max(f64::MIN_POSITIVE);
        Duration::from_micros(micros.ceil() as u64)
    }

    fn scrub_due(&self) -> bool {
        self.last_exact
            .is_none_or(|last| last.elapsed() >= SCRUB_INTERVAL)
    }

    /// Shows `frame` while the playhead is dragged.
    fn scrub(&mut self, frame: Frame) {
        match self.shown_at(frame, Fetch::Cached) {
            Ok(Shown::Unknown) => {}
            Ok(shown) => {
                self.pending_scrub = None;
                return self.present(frame, shown, true);
            }
            Err(error) => return self.fail(error),
        }
        if self.scrub_due() {
            return self.show_exact(frame);
        }
        if let Ok(shown @ Shown::Picture(_)) = self.shown_at(frame, Fetch::Nearest) {
            self.present(frame, shown, false);
        }
        self.target = Some(frame);
        self.pending_scrub = Some(frame);
    }

    /// Shows exactly `frame`, decoding it if needed; while an export runs, the nearest
    /// cached frame instead.
    fn show_exact(&mut self, frame: Frame) {
        self.pending_scrub = None;
        self.last_exact = Some(Instant::now());
        let exporting = self.exporting.load(Ordering::Relaxed);
        let fetch = if exporting {
            Fetch::Nearest
        } else {
            Fetch::Decode
        };
        match self.shown_at(frame, fetch) {
            Ok(shown) => self.present(frame, shown, !exporting),
            Err(error) => self.fail(error),
        }
    }

    /// Shows the frame the playback clock is at, and stops at the ends of the sequence.
    fn follow_clock(&mut self, generation: u64) {
        let (time, factor) = {
            let transport = lock(&self.transport);
            let current = transport.generation == generation;
            (
                transport.clock.time(Instant::now()),
                transport.playing.filter(|_| current),
            )
        };
        let (Some(factor), Some(project)) = (factor, self.project.clone()) else {
            self.playing = None;
            return;
        };
        let frame = frame_at(MediaTime(time), project.sequence().frame_rate());
        let last = (project.sequence().end() - Frame(1)).max(Frame(0));
        let forward = factor > 0.0;
        if forward && frame > last {
            return self.stop_at(generation, last);
        }
        if !forward && frame < Frame(0) {
            return self.stop_at(generation, Frame(0));
        }
        if self.shown_exactly == Some(frame) {
            return;
        }
        let fetch = if forward { Fetch::Decode } else { Fetch::Gop };
        match self.shown_at(frame, fetch) {
            Ok(shown) => self.present(frame, shown, true),
            Err(error) => {
                self.fail(error);
                return self.stop_at(generation, frame);
            }
        }
        // Get the next frame ready while this one is on screen.
        let step = (factor.abs().round() as i64).max(1);
        let next = if forward {
            frame + Frame(step)
        } else {
            frame - Frame(step)
        };
        if (Frame(0)..=last).contains(&next) {
            let _ = self.shown_at(next, fetch);
        }
    }

    /// Ends playback `generation` at `frame`, if it is still the current playback.
    fn stop_at(&mut self, generation: u64, frame: Frame) {
        self.playing = None;
        {
            let mut transport = lock(&self.transport);
            if transport.generation != generation {
                return;
            }
            transport.generation += 1;
            transport.playing = None;
            let at = frame_to_media(frame, transport.rate);
            transport.clock = PlaybackClock::stopped(at.0);
        }
        if self.shown_exactly != Some(frame) {
            self.show_exact(frame);
        }
        (self.report)(EngineEvent::Stopped { frame });
    }

    /// What the preview shows at `frame`, going as far as `fetch` allows.
    fn shown_at(&mut self, frame: Frame, fetch: Fetch) -> Result<Shown, EngineError> {
        let Some(project) = self.project.clone() else {
            return Ok(Shown::Black);
        };
        let sequence = project.sequence();
        let Some(clip) = sequence.visible_video_at(frame) else {
            return Ok(Shown::Black);
        };
        let (media, time) = (
            clip.media_id,
            clip.source_time_at(frame, sequence.frame_rate()),
        );
        if let Some(picture) = self.cache.get(media, time) {
            return Ok(Shown::Picture(picture));
        }
        match fetch {
            Fetch::Cached => Ok(Shown::Unknown),
            Fetch::Nearest => Ok(self
                .cache
                .nearest(media, time)
                .map_or(Shown::Unknown, Shown::Picture)),
            Fetch::Decode => self.decode(&project, media, time),
            Fetch::Gop => self.decode_gop(&project, media, time),
        }
    }

    /// Decodes the frame of `media` shown at `time` into the cache.
    fn decode(
        &mut self,
        project: &Project,
        media: MediaId,
        time: MediaTime,
    ) -> Result<Shown, EngineError> {
        let index = self.decoder(project, media)?;
        let decoder = &mut self.decoders[index].decoder;
        let Some(decoded) = decoder.frame_at(time)? else {
            // Before the first frame of the stream.
            return Ok(Shown::Black);
        };
        let following = decoder.following();
        let picture = self
            .cache
            .insert(media, decoded.time, following, decoded.picture);
        Ok(Shown::Picture(picture))
    }

    /// Decodes every frame of `media` from the keyframe before `time` up to the frame shown
    /// at `time` into the cache, so that playing backwards finds the frames before it there.
    fn decode_gop(
        &mut self,
        project: &Project,
        media: MediaId,
        time: MediaTime,
    ) -> Result<Shown, EngineError> {
        let index = self.decoder(project, media)?;
        let decoder = &mut self.decoders[index].decoder;
        decoder.seek(time)?;
        let mut at = match decoder.next_frame()? {
            Some(first) if first.time <= time => first.time,
            _ => return Ok(Shown::Black),
        };
        loop {
            let Some(decoded) = decoder.frame_at(at)? else {
                return Ok(Shown::Black);
            };
            let following = decoder.following();
            let picture = self
                .cache
                .insert(media, decoded.time, following, decoded.picture);
            match following {
                Following::Next(next) if next <= time && next > at => at = next,
                _ => return Ok(Shown::Picture(picture)),
            }
        }
    }

    /// The index of an open decoder for `media`, opened if needed within the decoder budget.
    fn decoder(&mut self, project: &Project, media: MediaId) -> Result<usize, EngineError> {
        let now = Instant::now();
        if let Some(index) = self.decoders.iter().position(|open| open.media == media) {
            self.decoders[index].last_used = now;
            return Ok(index);
        }
        let Some(media_ref) = project.media_ref(media) else {
            return Err(EngineError::Unsupported {
                path: Default::default(),
                reason: "the project lists a clip without its media file",
            });
        };
        let info = &media_ref.info;
        let large = u64::from(info.width) * u64::from(info.height) > LARGE_FRAME;
        // At most two video decoders, or one for a source above the 1080p class
        // (docs/ARCHITECTURE.md, "Decoder pool").
        if large || self.decoders.iter().any(|open| open.large) {
            self.decoders.clear();
        }
        while self.decoders.len() >= 2 {
            let oldest = (0..self.decoders.len())
                .min_by_key(|index| self.decoders[*index].last_used)
                .unwrap_or(0);
            self.decoders.remove(oldest);
        }
        // Software decoding in 0.1 (docs/ARCHITECTURE.md, "Decoder pool").
        let decoder = VideoDecoder::open(&media_ref.path, Acceleration::Software)?;
        self.decoders.push(OpenDecoder {
            media,
            decoder,
            large,
            last_used: now,
        });
        self.open_decoders
            .store(self.decoders.len(), Ordering::Relaxed);
        Ok(self.decoders.len() - 1)
    }

    /// Draws what is shown at `frame` and reports it; `exact` says whether it is that frame
    /// or a stand-in while scrubbing.
    fn present(&mut self, frame: Frame, shown: Shown, exact: bool) {
        self.target = Some(frame);
        let texture = match shown {
            Shown::Unknown => return,
            Shown::Black => None,
            // Nothing to draw into before the preview has a size.
            Shown::Picture(_) if self.size.0 == 0 || self.size.1 == 0 => return,
            Shown::Picture(picture) => match self.compositor.render(&picture, self.size) {
                Ok(texture) => Some(texture),
                Err(error) => return self.fail(error.into()),
            },
        };
        self.shown_exactly = exact.then_some(frame);
        (self.report)(EngineEvent::Frame { frame, texture });
    }

    fn fail(&self, error: EngineError) {
        (self.report)(EngineEvent::Error(error));
    }
}
