//! The video thread (docs/ARCHITECTURE.md, "Data flow" and "Decoder pool"): it finds the clip
//! visible at a timeline frame, takes that clip's frame from the cache or decodes it, draws it
//! at the preview size and reports it. It serves two previews, the main window's and the clip
//! editor's, each with its own project, through one frame cache and one decoder pool. While
//! one of them plays it follows the playback clock, and a worker thread gets the next clip to
//! come into view ready.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use dusk_audio::PlaybackClock;
use dusk_core::time::{frame_at, frame_to_media};
use dusk_core::{ClipId, Frame, MediaId, MediaInfo, MediaKind, MediaTime, Picture, Project};
use dusk_media::{Acceleration, DecodedFrame, Following, LARGE_FRAME, Step, VideoDecoder};
use dusk_render::{Compositor, Gpu, fit_size};

use crate::EngineError;
use crate::cache::FrameCache;
use crate::engine::{EngineEvent, EngineOptions, Preview, Report, SharedTransport, lock};
use crate::info::still_size;
use crate::placement::placement_at;

/// What the front asks of the video thread, for one of the previews.
pub(crate) enum VideoRequest {
    Project(Preview, Arc<Project>),
    Size(Preview, (u32, u32)),
    Show(Preview, Frame),
    Scrub(Preview, Frame),
    Play {
        preview: Preview,
        generation: u64,
    },
    /// The preview's window closed: what it showed and drew with is let go.
    Close(Preview),
    /// The frame cache holds at most this many bytes from now on.
    CacheCap(usize),
}

/// While scrubbing, exact frames are decoded at most this often.
const SCRUB_INTERVAL: Duration = Duration::from_millis(16);
/// While playing, the clock is read at least this often.
const PLAYBACK_POLL: Duration = Duration::from_millis(10);
/// While playing, the next clip to come into view within this much playback is made ready
/// (docs/ARCHITECTURE.md, "Decoder pool").
const LOOKAHEAD: Duration = Duration::from_secs(2);

/// Whether a source is above the 1080p class, so that it gets a decoder to itself.
fn is_large(info: &MediaInfo) -> bool {
    u64::from(info.width) * u64::from(info.height) > LARGE_FRAME
}

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
            // The main window's compositor is there from the start, so its first frame does
            // not wait for one; the clip editor's is made when it first draws.
            let main = View {
                compositor: Some(Compositor::new(&gpu)),
                ..View::default()
            };
            VideoThread {
                gpu,
                transport,
                report,
                open_decoders,
                exporting,
                idle,
                views: [main, View::default()],
                cache: FrameCache::new(cap),
                decoders: Vec::new(),
                upcoming: None,
                lookahead_busy: Arc::new(AtomicBool::new(false)),
                playing: None,
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
    /// Take the keyframe at or before it, for playing too fast to decode every frame.
    Keyframe,
}

/// Where the frames kept from a group of pictures decoded for playing backwards start: as
/// many as fit in half of a cache of `cap` bytes, frames of `frame_bytes` each and `frame`
/// long, ending with the one at `time`; at least that one (docs/ARCHITECTURE.md, "Playback").
fn reverse_window(time: MediaTime, frame_bytes: usize, cap: usize, frame: MediaTime) -> MediaTime {
    let kept = (cap / 2 / frame_bytes.max(1)).max(1);
    let before = i64::try_from(kept - 1).unwrap_or(i64::MAX);
    MediaTime(time.0.saturating_sub(before.saturating_mul(frame.0)))
}

/// How frames are fetched while playing at `rate`, the playback factor times the clip's
/// speed (docs/ARCHITECTURE.md, "Playback"): every frame forwards up to 8x, every frame
/// backwards from a group-of-pictures buffer up to 2x, and only keyframes beyond either.
fn playback_fetch(rate: f64) -> Fetch {
    if rate > 8.0 || rate < -2.0 {
        Fetch::Keyframe
    } else if rate > 0.0 {
        Fetch::Decode
    } else {
        Fetch::Gop
    }
}

struct OpenDecoder {
    media: MediaId,
    decoder: VideoDecoder,
    large: bool,
    last_used: Instant,
}

/// The next clip to come into view while playing, made ready by a worker thread so that
/// playback does not wait for it: the clip's own decoder opened and moved to the frame the
/// clip comes into view with, and that frame in the cache.
struct Upcoming {
    clip: ClipId,
    media: MediaId,
    /// The time in its source of the frame it comes into view with.
    time: MediaTime,
    state: Readiness,
}

/// How far the upcoming clip is.
enum Readiness {
    /// Not started: one worker at a time, and an earlier one is still busy.
    Waiting,
    /// A worker is on it.
    Working(Receiver<Prepared>),
    /// Done. A video clip's decoder joins the pool when the clip comes into view.
    Ready(Option<VideoDecoder>),
}

/// What a lookahead worker made ready.
enum Prepared {
    /// A video clip's decoder at the frame the clip comes into view with, that frame, and when
    /// the one after it starts.
    Video {
        decoder: VideoDecoder,
        frame: Option<DecodedFrame>,
        following: Following,
    },
    /// A still's picture at the size the sequence needs.
    Still(Picture),
}

/// Marks the lookahead worker busy for as long as it lives, even if it panics.
struct Busy(Arc<AtomicBool>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// What one preview shows and draws with.
#[derive(Default)]
struct View {
    project: Option<Arc<Project>>,
    size: (u32, u32),
    /// Its own compositor: a compositor's output frames stay intact only until two newer
    /// ones are drawn, which another preview's drawing must not count toward.
    compositor: Option<Compositor>,
    /// The frame it should show.
    target: Option<Frame>,
    /// The frame it shows exactly, if it does.
    shown_exactly: Option<Frame>,
    /// A scrub position shown only approximately so far.
    pending_scrub: Option<Frame>,
    last_exact: Option<Instant>,
}

struct VideoThread {
    gpu: Gpu,
    transport: SharedTransport,
    report: Report,
    open_decoders: Arc<AtomicUsize>,
    /// While an export runs, the previews show cached frames only.
    exporting: Arc<AtomicBool>,
    idle: Duration,
    /// The previews, by [`Preview::index`].
    views: [View; 2],
    cache: FrameCache,
    decoders: Vec<OpenDecoder>,
    /// While playing forwards, the next clip to come into view.
    upcoming: Option<Upcoming>,
    /// A lookahead worker is running, holding a decoder; there is one at a time.
    lookahead_busy: Arc<AtomicBool>,
    /// The playback this thread follows, by generation, and the preview it plays in.
    playing: Option<(u64, Preview)>,
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
            // Requests pile up while a frame is decoded; only the newest frame request of each
            // preview counts.
            let mut wanted: [Option<(Frame, bool)>; 2] = [None; 2];
            let mut redraw = [false; 2];
            for request in first.into_iter().chain(inbox.try_iter()) {
                match request {
                    VideoRequest::Project(preview, project) => {
                        self.forget_moved_media(preview, &project);
                        let view = self.view_mut(preview);
                        view.project = Some(project);
                        // An edit can change what any frame shows, and what comes next.
                        view.shown_exactly = None;
                        self.upcoming = None;
                    }
                    VideoRequest::Size(preview, size) => {
                        self.view_mut(preview).size = size;
                        redraw[preview.index()] = true;
                    }
                    VideoRequest::Show(preview, frame) => {
                        self.playing = None;
                        self.upcoming = None;
                        wanted[preview.index()] = Some((frame, true));
                    }
                    VideoRequest::Scrub(preview, frame) => {
                        self.playing = None;
                        self.upcoming = None;
                        wanted[preview.index()] = Some((frame, false));
                    }
                    VideoRequest::Play {
                        preview,
                        generation,
                    } => {
                        self.playing = Some((generation, preview));
                        self.view_mut(preview).pending_scrub = None;
                        wanted[preview.index()] = None;
                    }
                    VideoRequest::Close(preview) => {
                        if self.playing.is_some_and(|(_, playing)| playing == preview) {
                            self.playing = None;
                        }
                        *self.view_mut(preview) = View::default();
                        (wanted[preview.index()], redraw[preview.index()]) = (None, false);
                    }
                    VideoRequest::CacheCap(cap) => self.cache.set_cap(cap),
                }
            }
            for preview in Preview::ALL {
                let index = preview.index();
                if let Some((generation, playing)) = self.playing
                    && playing == preview
                {
                    self.follow_clock(preview, generation);
                } else if let Some((frame, exact)) = wanted[index] {
                    if exact {
                        self.show_exact(preview, frame);
                    } else {
                        self.scrub(preview, frame);
                    }
                } else if let Some(frame) = self
                    .view(preview)
                    .pending_scrub
                    .filter(|_| self.scrub_due(preview))
                {
                    self.show_exact(preview, frame);
                } else if let Some(frame) = self.view(preview).target.filter(|_| redraw[index]) {
                    self.show_exact(preview, frame);
                }
            }
            if self.playing.is_none() {
                self.upcoming = None;
            }
            let now = Instant::now();
            let idle = self.idle;
            self.decoders
                .retain(|open| now.duration_since(open.last_used) < idle);
            self.open_decoders
                .store(self.decoders_open(), Ordering::Relaxed);
        }
    }

    fn view(&self, preview: Preview) -> &View {
        &self.views[preview.index()]
    }

    fn view_mut(&mut self, preview: Preview) -> &mut View {
        &mut self.views[preview.index()]
    }

    /// The video decoders open: the pool's, the upcoming clip's, and a lookahead worker's.
    fn decoders_open(&self) -> usize {
        let ready = self
            .upcoming
            .as_ref()
            .is_some_and(|upcoming| matches!(upcoming.state, Readiness::Ready(Some(_))));
        let working = self.lookahead_busy.load(Ordering::Acquire);
        self.decoders.len() + usize::from(ready) + usize::from(working)
    }

    /// How long to wait for a request before there is work of its own; `None` for as long as
    /// it takes.
    fn wait(&self) -> Option<Duration> {
        if let Some((_, preview)) = self.playing {
            return Some(self.until_next_frame(preview).min(PLAYBACK_POLL));
        }
        let now = Instant::now();
        let scrub = self
            .views
            .iter()
            .filter(|view| view.pending_scrub.is_some())
            .map(|view| {
                view.last_exact.map_or(Duration::ZERO, |last| {
                    (last + SCRUB_INTERVAL).saturating_duration_since(now)
                })
            })
            .min();
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

    /// How long until the playback clock reaches the next frame of `preview`, the one
    /// playing; zero when the frame on screen is not the one the clock is at.
    fn until_next_frame(&self, preview: Preview) -> Duration {
        let transport = lock(&self.transport);
        let (Some(factor), true) = (transport.playing, transport.clock.is_running()) else {
            // Waiting for the sound to start the clock.
            return Duration::from_millis(1);
        };
        let rate = transport.rate();
        let time = transport.clock.time(Instant::now());
        let frame = frame_at(MediaTime(time), rate);
        if self.view(preview).shown_exactly != Some(frame) {
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

    fn scrub_due(&self, preview: Preview) -> bool {
        self.view(preview)
            .last_exact
            .is_none_or(|last| last.elapsed() >= SCRUB_INTERVAL)
    }

    /// Shows `frame` in `preview` while its playhead is dragged.
    fn scrub(&mut self, preview: Preview, frame: Frame) {
        match self.shown_at(preview, frame, Fetch::Cached) {
            Ok(Shown::Unknown) => {}
            Ok(shown) => {
                self.view_mut(preview).pending_scrub = None;
                return self.present(preview, frame, shown, true);
            }
            Err(error) => return self.fail_at(preview, frame, error),
        }
        if self.scrub_due(preview) {
            return self.show_exact(preview, frame);
        }
        if let Ok(shown @ Shown::Picture(_)) = self.shown_at(preview, frame, Fetch::Nearest) {
            self.present(preview, frame, shown, false);
        }
        let view = self.view_mut(preview);
        view.target = Some(frame);
        view.pending_scrub = Some(frame);
    }

    /// Shows exactly `frame` in `preview`, decoding it if needed; while an export runs, the
    /// nearest cached frame instead.
    fn show_exact(&mut self, preview: Preview, frame: Frame) {
        let view = self.view_mut(preview);
        view.pending_scrub = None;
        view.last_exact = Some(Instant::now());
        let exporting = self.exporting.load(Ordering::Relaxed);
        let fetch = if exporting {
            Fetch::Nearest
        } else {
            Fetch::Decode
        };
        match self.shown_at(preview, frame, fetch) {
            Ok(shown) => self.present(preview, frame, shown, !exporting),
            Err(error) => self.fail_at(preview, frame, error),
        }
    }

    /// Shows in `preview` the frame the playback clock is at, and stops at the ends of its
    /// sequence.
    fn follow_clock(&mut self, preview: Preview, generation: u64) {
        let (time, factor) = {
            let transport = lock(&self.transport);
            let current = transport.generation == generation;
            (
                transport.clock.time(Instant::now()),
                transport.playing.filter(|_| current),
            )
        };
        let (Some(factor), Some(project)) = (factor, self.view(preview).project.clone()) else {
            self.playing = None;
            return;
        };
        let frame = frame_at(MediaTime(time), project.sequence().frame_rate());
        let last = (project.sequence().end() - Frame(1)).max(Frame(0));
        let forward = factor > 0.0;
        if forward && frame > last {
            return self.stop_at(preview, generation, last);
        }
        if !forward && frame < Frame(0) {
            return self.stop_at(preview, generation, Frame(0));
        }
        if self.view(preview).shown_exactly == Some(frame) {
            return self.tend_upcoming(preview, &project);
        }
        let speed = project
            .sequence()
            .visible_video_at(frame)
            .map_or(1.0, |clip| clip.speed);
        let fetch = playback_fetch(factor * speed);
        match self.shown_at(preview, frame, fetch) {
            Ok(shown) => self.present(preview, frame, shown, true),
            Err(error) => {
                self.fail(error);
                return self.stop_at(preview, generation, frame);
            }
        }
        // Get the next frame ready while this one is on screen; keyframes come as they come.
        if matches!(fetch, Fetch::Keyframe) {
            self.upcoming = None;
            return;
        }
        let step = (factor.abs().round() as i64).max(1);
        let next = if forward {
            frame + Frame(step)
        } else {
            frame - Frame(step)
        };
        if (Frame(0)..=last).contains(&next) {
            let _ = self.shown_at(preview, next, fetch);
        }
        // Playing forwards, the clip to come into view after that one, too.
        if forward && matches!(fetch, Fetch::Decode) {
            self.plan_upcoming(&project, next.min(last), factor);
            self.tend_upcoming(preview, &project);
        } else {
            self.upcoming = None;
        }
    }

    /// Picks the next clip to come into view within [`LOOKAHEAD`] of playback after `from`,
    /// for a worker to make ready. A source above the 1080p class, in view or coming, means no
    /// lookahead: it may have the only decoder.
    fn plan_upcoming(&mut self, project: &Project, from: Frame, factor: f64) {
        let sequence = project.sequence();
        let rate = sequence.frame_rate();
        let ahead = LOOKAHEAD.as_secs_f64() * factor.max(1.0);
        let within = frame_at(MediaTime((ahead * 1e6) as i64), rate);
        let Some((at, clip)) = sequence.next_visible_video(from, within) else {
            self.upcoming = None;
            return;
        };
        if self
            .upcoming
            .as_ref()
            .is_some_and(|upcoming| upcoming.clip == clip.id)
        {
            return;
        }
        let large = |media: MediaId| {
            project
                .media_ref(media)
                .is_some_and(|media| is_large(&media.info))
        };
        let in_view = sequence.visible_video_at(from).map(|clip| clip.media_id);
        if large(clip.media_id) || in_view.is_some_and(large) {
            self.upcoming = None;
            return;
        }
        self.upcoming = Some(Upcoming {
            clip: clip.id,
            media: clip.media_id,
            time: clip.source_time_at(at, rate),
            state: Readiness::Waiting,
        });
    }

    /// Starts a worker on the upcoming clip of `preview`, the one playing, when none is busy,
    /// and takes what it made ready once it has.
    fn tend_upcoming(&mut self, preview: Preview, project: &Project) {
        match self.upcoming.as_ref().map(|upcoming| &upcoming.state) {
            Some(Readiness::Waiting) if !self.lookahead_busy.load(Ordering::Acquire) => {
                self.start_upcoming(preview, project);
            }
            Some(Readiness::Working(_)) => self.receive_upcoming(false),
            _ => {}
        }
    }

    /// Hands the upcoming clip to a worker thread. Its decoder takes the place of every
    /// decoder in the pool but the one of the clip in view, so that at most two are open; a
    /// decoder of the same media kept for a clip out of view goes with it.
    fn start_upcoming(&mut self, preview: Preview, project: &Project) {
        let in_view = self
            .view(preview)
            .shown_exactly
            .and_then(|frame| project.sequence().visible_video_at(frame))
            .map(|clip| clip.media_id);
        let Some(upcoming) = self.upcoming.as_mut() else {
            return;
        };
        let Some(media_ref) = project.media_ref(upcoming.media) else {
            upcoming.state = Readiness::Ready(None);
            return;
        };
        let (media, time, path) = (upcoming.media, upcoming.time, media_ref.path.clone());
        let job: Box<dyn FnOnce() -> Option<Prepared> + Send> =
            if media_ref.info.kind == MediaKind::Still {
                let size = still_size(&media_ref.info, project.sequence().resolution());
                let cached = self.cache.get(media, MediaTime(0));
                if cached.is_some_and(|picture| (picture.width, picture.height) == size) {
                    upcoming.state = Readiness::Ready(None);
                    return;
                }
                Box::new(move || {
                    dusk_media::decode_still(&path, size)
                        .ok()
                        .map(Prepared::Still)
                })
            } else {
                let spare = self
                    .decoders
                    .iter()
                    .position(|open| open.media == media && in_view != Some(media));
                let spare = spare.map(|index| self.decoders.remove(index).decoder);
                self.decoders.retain(|open| in_view == Some(open.media));
                Box::new(move || {
                    let mut decoder = match spare {
                        Some(decoder) => decoder,
                        None => VideoDecoder::open(&path, Acceleration::Software).ok()?,
                    };
                    let frame = decoder.frame_at(time).ok()?;
                    let following = decoder.following();
                    Some(Prepared::Video {
                        decoder,
                        frame,
                        following,
                    })
                })
            };
        let (sender, receiver) = crossbeam_channel::bounded(1);
        self.lookahead_busy.store(true, Ordering::Release);
        let busy = Busy(Arc::clone(&self.lookahead_busy));
        let spawned = std::thread::Builder::new()
            .name("dusk lookahead".to_owned())
            .spawn(move || {
                let prepared = job();
                drop(busy);
                // Nobody is waiting any more if playback moved on meanwhile.
                if let Some(prepared) = prepared {
                    let _ = sender.send(prepared);
                }
            });
        // Without a thread there is no lookahead; the clip is decoded when it is due.
        upcoming.state = match spawned {
            Ok(_) => Readiness::Working(receiver),
            Err(_) => Readiness::Ready(None),
        };
    }

    /// Takes what the worker made ready, waiting for it when `wait` is set; a worker that
    /// failed leaves the clip to be decoded when it is due, which reports the error.
    fn receive_upcoming(&mut self, wait: bool) {
        let Some(upcoming) = self.upcoming.as_mut() else {
            return;
        };
        let Readiness::Working(receiver) = &upcoming.state else {
            return;
        };
        let prepared = if wait {
            receiver.recv().ok()
        } else {
            match receiver.try_recv() {
                Ok(prepared) => Some(prepared),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => None,
            }
        };
        let decoder = match prepared {
            Some(Prepared::Video {
                decoder,
                frame,
                following,
            }) => {
                if let Some(frame) = frame {
                    self.cache
                        .insert(upcoming.media, frame.time, following, frame.picture);
                }
                Some(decoder)
            }
            Some(Prepared::Still(picture)) => {
                self.cache
                    .insert(upcoming.media, MediaTime(0), Following::End, picture);
                None
            }
            None => None,
        };
        upcoming.state = Readiness::Ready(decoder);
    }

    /// The upcoming clip comes into view: its decoder joins the pool, in place of any other
    /// for the same media. A worker still busy with it is waited for, being the quickest way
    /// to its frame.
    fn take_upcoming(&mut self) {
        self.receive_upcoming(true);
        let Some(upcoming) = self.upcoming.take() else {
            return;
        };
        if let Readiness::Ready(Some(decoder)) = upcoming.state {
            self.decoders.retain(|open| open.media != upcoming.media);
            self.decoders.push(OpenDecoder {
                media: upcoming.media,
                decoder,
                large: false,
                last_used: Instant::now(),
            });
        }
    }

    /// Ends playback `generation`, in `preview`, at `frame`, if it is still the current
    /// playback.
    fn stop_at(&mut self, preview: Preview, generation: u64, frame: Frame) {
        self.playing = None;
        {
            let mut transport = lock(&self.transport);
            if transport.generation != generation {
                return;
            }
            transport.generation += 1;
            transport.playing = None;
            let at = frame_to_media(frame, transport.rate());
            transport.clock = PlaybackClock::stopped(at.0);
        }
        if self.view(preview).shown_exactly != Some(frame) {
            self.show_exact(preview, frame);
        }
        (self.report)(EngineEvent::Stopped { preview, frame });
    }

    /// What `preview` shows at `frame`, going as far as `fetch` allows.
    fn shown_at(
        &mut self,
        preview: Preview,
        frame: Frame,
        fetch: Fetch,
    ) -> Result<Shown, EngineError> {
        let Some(project) = self.view(preview).project.clone() else {
            return Ok(Shown::Black);
        };
        let sequence = project.sequence();
        let Some(clip) = sequence.visible_video_at(frame) else {
            return Ok(Shown::Black);
        };
        if self
            .upcoming
            .as_ref()
            .is_some_and(|upcoming| upcoming.clip == clip.id)
        {
            self.take_upcoming();
        }
        let still = project
            .media_ref(clip.media_id)
            .is_some_and(|media| media.info.kind == MediaKind::Still);
        if still {
            return self.still(&project, clip.media_id, fetch);
        }
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
            Fetch::Keyframe => self.decode_keyframe(&project, media, time),
        }
    }

    /// Decodes the keyframe of `media` at or before `time` into the cache, without the
    /// frames after it.
    fn decode_keyframe(
        &mut self,
        project: &Project,
        media: MediaId,
        time: MediaTime,
    ) -> Result<Shown, EngineError> {
        let index = self.decoder(project, media)?;
        let decoder = &mut self.decoders[index].decoder;
        decoder.seek(time)?;
        let Some(decoded) = decoder.next_frame()? else {
            return Ok(Shown::Black);
        };
        let picture = self
            .cache
            .insert(media, decoded.time, Following::Unknown, decoded.picture);
        Ok(Shown::Picture(picture))
    }

    /// The picture of the still `media`, at the size the sequence needs: cached once at time
    /// 0, covering every time, and decoded again when that size changes. While `fetch` allows
    /// no decoding, any cached size stands in.
    fn still(
        &mut self,
        project: &Project,
        media: MediaId,
        fetch: Fetch,
    ) -> Result<Shown, EngineError> {
        let Some(media_ref) = project.media_ref(media) else {
            return Ok(Shown::Black);
        };
        let size = still_size(&media_ref.info, project.sequence().resolution());
        let cached = self.cache.get(media, MediaTime(0));
        let decode = matches!(fetch, Fetch::Decode | Fetch::Gop);
        match cached {
            Some(picture) if (picture.width, picture.height) == size || !decode => {
                return Ok(Shown::Picture(picture));
            }
            None if !decode => return Ok(Shown::Unknown),
            _ => {}
        }
        let picture = dusk_media::decode_still(&media_ref.path, size)?;
        let picture = self
            .cache
            .insert(media, MediaTime(0), Following::End, picture);
        Ok(Shown::Picture(picture))
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

    /// Decodes the frames of `media` from the keyframe before `time` up to the frame shown at
    /// `time` into the cache, so that playing backwards finds the frames before it there. Of
    /// a group too long for half the cache, only the last frames that fit are kept; the ones
    /// before them are decoded without a copy, and decoded again from the keyframe once
    /// playback gets to them.
    fn decode_gop(
        &mut self,
        project: &Project,
        media: MediaId,
        time: MediaTime,
    ) -> Result<Shown, EngineError> {
        // The length of a frame, from the snapped rate, to count frames back from `time`.
        let frame = project
            .media_ref(media)
            .and_then(|media| media.info.frame_rate)
            .map_or(MediaTime(33_333), |rate| frame_to_media(Frame(1), rate));
        let cap = self.cache.cap();
        let index = self.decoder(project, media)?;
        let decoder = &mut self.decoders[index].decoder;
        decoder.seek(time)?;
        let first = match decoder.next_frame()? {
            Some(first) if first.time <= time => first,
            _ => return Ok(Shown::Black),
        };
        let start = reverse_window(time, first.picture.byte_size(), cap, frame);
        let mut at = first.time;
        if at < start {
            at = loop {
                match decoder.step_to(start)? {
                    Step::Working => {}
                    Step::Done(Some(frame)) => break frame.time,
                    Step::Done(None) => return Ok(Shown::Black),
                }
            };
        }
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

    /// Lets go of what was decoded from a media file that `project` finds somewhere else than
    /// the project `preview` showed before, as after relinking it: its frames and its decoder
    /// came from the old place.
    fn forget_moved_media(&mut self, preview: Preview, project: &Project) {
        let Some(before) = self.view(preview).project.clone() else {
            return;
        };
        for media in project.media() {
            let moved = before
                .media_ref(media.id)
                .is_some_and(|old| old.path != media.path);
            if moved {
                self.cache.forget(media.id);
                self.decoders.retain(|open| open.media != media.id);
                if self
                    .upcoming
                    .as_ref()
                    .is_some_and(|upcoming| upcoming.media == media.id)
                {
                    self.upcoming = None;
                }
            }
        }
        self.open_decoders
            .store(self.decoders_open(), Ordering::Relaxed);
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
                reason: "the project lists a clip without its media file; open the project again",
            });
        };
        let large = is_large(&media_ref.info);
        // At most two video decoders, the upcoming clip's included, or one for a source above
        // the 1080p class (docs/ARCHITECTURE.md, "Decoder pool").
        if large || self.decoders.iter().any(|open| open.large) {
            self.decoders.clear();
            self.upcoming = None;
        }
        while self.decoders_open() >= 2 && !self.decoders.is_empty() {
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
            .store(self.decoders_open(), Ordering::Relaxed);
        Ok(self.decoders.len() - 1)
    }

    /// Draws what `preview` shows at `frame` and reports it; `exact` says whether it is that
    /// frame or a stand-in while scrubbing.
    fn present(&mut self, preview: Preview, frame: Frame, shown: Shown, exact: bool) {
        let drawn = {
            let gpu = &self.gpu;
            let view = &mut self.views[preview.index()];
            view.target = Some(frame);
            // Nothing to draw into before the preview has a size.
            if matches!(shown, Shown::Unknown) || view.size.0 == 0 || view.size.1 == 0 {
                return;
            }
            // The sequence's frame, as large as fits in the preview; the preview's own
            // background shows around it.
            let project = view.project.as_deref();
            let size = project.map_or(view.size, |project| {
                fit_size(project.sequence().resolution(), view.size)
            });
            let compositor = view.compositor.get_or_insert_with(|| Compositor::new(gpu));
            match shown {
                Shown::Picture(picture) => {
                    let placement = project
                        .map(|project| placement_at(project, frame))
                        .unwrap_or_default();
                    compositor.render_placed(&picture, &placement, size)
                }
                Shown::Black | Shown::Unknown => compositor.blank(size),
            }
        };
        let texture = match drawn {
            Ok(texture) => Some(texture),
            Err(error) => return self.fail(error.into()),
        };
        self.view_mut(preview).shown_exactly = exact.then_some(frame);
        (self.report)(EngineEvent::Frame {
            preview,
            frame,
            texture,
        });
    }

    fn fail(&self, error: EngineError) {
        (self.report)(EngineEvent::Error(error));
    }

    /// `frame` of `preview` cannot be shown: says why, and shows it black, as a clip whose
    /// file is missing is, rather than leaving the picture shown before.
    fn fail_at(&mut self, preview: Preview, frame: Frame, error: EngineError) {
        self.fail(error);
        self.present(preview, frame, Shown::Black, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(fetch: Fetch) -> &'static str {
        match fetch {
            Fetch::Cached => "cached",
            Fetch::Nearest => "nearest",
            Fetch::Decode => "every frame",
            Fetch::Gop => "groups of pictures",
            Fetch::Keyframe => "keyframes",
        }
    }

    #[test]
    fn a_long_group_keeps_only_what_fits_in_half_the_cache() {
        let frame = MediaTime(33_333);
        // 10 MB of cache, 1 MB frames: 5 frames, the one asked for and 4 before it.
        let start = reverse_window(MediaTime(1_000_000), 1_000_000, 10_000_000, frame);
        assert_eq!(start, MediaTime(1_000_000 - 4 * 33_333));
        // Always at least the frame asked for.
        let start = reverse_window(MediaTime(1_000_000), 8_000_000, 10_000_000, frame);
        assert_eq!(start, MediaTime(1_000_000));
    }

    #[test]
    fn fast_playback_takes_only_keyframes() {
        let cases = [
            (1.0, "every frame"),
            (8.0, "every frame"),
            (16.0, "keyframes"),
            (0.1, "every frame"),
            (-1.0, "groups of pictures"),
            (-2.0, "groups of pictures"),
            (-4.0, "keyframes"),
        ];
        for (rate, expected) in cases {
            assert_eq!(kind(playback_fetch(rate)), expected, "at {rate}x");
        }
    }
}
