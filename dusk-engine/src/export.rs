//! The export pipeline (docs/ARCHITECTURE.md, "Export"): on its own thread, from a snapshot of
//! the project, every timeline frame is drawn at the sequence size, turned into YUV on the
//! GPU, read back and encoded, and the sound is mixed offline beside it. The file is written
//! as `name.ext.part` and renamed when it is complete, so a cancelled or failed export never
//! leaves a broken file where the user expects a video.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dusk_core::time::frame_to_media;
use dusk_core::{Frame, MediaId, MediaKind, MediaTime, Picture, Project, Rational};
use dusk_media::{
    Acceleration, AudioCodec, AudioFormat, Container, Quality, VideoCodec, VideoDecoder,
    VideoSettings, Writer,
};
use dusk_render::{Compositor, Gpu, ToYuv};

use crate::EngineError;
use crate::engine::{EngineEvent, Report};
use crate::external::{Encoding, ExternalEncoder, ExternalJob};
use crate::info::still_size;
use crate::mixer::Mixer;
use crate::placement::placement_at;
use crate::settings::{AUDIO_RATE, ExportFormat, ExportSettings, export_size, has_sound};
use crate::size::corrected;
use crate::transcode::part_path;

/// What an export reports, as [`EngineEvent::Export`].
#[derive(Debug)]
pub enum ExportEvent {
    /// `done` of `total` frames are written.
    Progress {
        /// Frames written so far.
        done: u64,
        /// Frames in the sequence.
        total: u64,
    },
    /// A file made to a size came out at `first` bytes, more than 3% over it; it is made
    /// again once, smaller, and the progress starts over (docs/ARCHITECTURE.md, "Target file
    /// size").
    Again {
        /// The first file's size, in bytes.
        first: u64,
    },
    /// The file is complete.
    Finished {
        /// Where it is.
        path: PathBuf,
        /// The video encoder that wrote it, such as `h264_amf`.
        encoder: String,
        /// Its size, in bytes.
        bytes: u64,
    },
    /// The export was cancelled; nothing was left behind.
    Cancelled,
    /// The export failed; nothing was left behind.
    Failed(EngineError),
}

/// A running export.
#[derive(Clone, Debug)]
pub struct ExportJob {
    cancel: Arc<AtomicBool>,
}

impl ExportJob {
    /// Stops the export; it reports [`ExportEvent::Cancelled`] once it has cleaned up.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Progress is reported at most this often.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// How an export writes its file.
#[derive(Clone, Debug)]
pub(crate) struct ExportPlan {
    pub settings: ExportSettings,
    /// A size to aim at: a file more than 3% over it is made again once, smaller.
    pub size: Option<u64>,
    /// A user's own `ffmpeg` to encode the video with, in place of Dusk's encoders.
    pub external: Option<ExternalEncoder>,
}

/// Starts exporting `project` to `path` on a new thread, which ends after reporting the
/// outcome.
pub(crate) fn spawn(
    gpu: &Gpu,
    project: Arc<Project>,
    path: PathBuf,
    plan: ExportPlan,
    exporting: Arc<AtomicBool>,
    report: Report,
) -> Result<(ExportJob, JoinHandle<()>), EngineError> {
    let cancel = Arc::new(AtomicBool::new(false));
    let job = ExportJob {
        cancel: Arc::clone(&cancel),
    };
    let gpu = gpu.clone();
    exporting.store(true, Ordering::Relaxed);
    let spawned = std::thread::Builder::new()
        .name("dusk export".to_owned())
        .spawn({
            let exporting = Arc::clone(&exporting);
            move || {
                let outcome = export(&gpu, &project, &path, &plan, &cancel, &report);
                exporting.store(false, Ordering::Relaxed);
                report(EngineEvent::Export(outcome));
            }
        });
    match spawned {
        Ok(thread) => Ok((job, thread)),
        Err(error) => {
            exporting.store(false, Ordering::Relaxed);
            Err(EngineError::Thread(error))
        }
    }
}

/// Exports, making the file again once when it comes out too far over the plan's size, then
/// renames the finished file or removes what was written.
fn export(
    gpu: &Gpu,
    project: &Arc<Project>,
    path: &Path,
    plan: &ExportPlan,
    cancel: &AtomicBool,
    report: &Report,
) -> ExportEvent {
    let part = part_path(path);
    let external = plan.external.as_ref();
    let settings = &plan.settings;
    let mut outcome = write_file(gpu, project, &part, settings, external, cancel, report);
    if let (Ok(Some(_)), Some(target), Quality::Bitrate(bit_rate)) =
        (&outcome, plan.size, settings.quality)
    {
        let first = std::fs::metadata(&part).map_or(0, |file| file.len());
        if let Some(lower) = corrected(bit_rate, target, first) {
            report(EngineEvent::Export(ExportEvent::Again { first }));
            let again = ExportSettings {
                quality: Quality::Bitrate(lower),
                ..*settings
            };
            outcome = write_file(gpu, project, &part, &again, external, cancel, report);
        }
    }
    let renamed = match outcome {
        Ok(Some(encoder)) => match std::fs::rename(&part, path) {
            Ok(()) => {
                return ExportEvent::Finished {
                    path: path.to_path_buf(),
                    encoder,
                    bytes: std::fs::metadata(path).map_or(0, |file| file.len()),
                };
            }
            Err(source) => Err(EngineError::Io {
                path: path.to_path_buf(),
                source,
            }),
        },
        Ok(None) => Ok(()),
        Err(error) => Err(error),
    };
    // Nothing half-written stays behind; the file may not even have been created.
    let _ = std::fs::remove_file(&part);
    match renamed {
        Ok(()) => ExportEvent::Cancelled,
        // A source on a drive that went away fails as FFmpeg failing to read it.
        Err(error) => ExportEvent::Failed(error.or_gone(Path::is_file)),
    }
}

/// Writes the sequence to `part` in `settings`' format, the video through `external` when
/// given; the encoder used, or `None` when cancelled.
fn write_file(
    gpu: &Gpu,
    project: &Arc<Project>,
    part: &Path,
    settings: &ExportSettings,
    external: Option<&ExternalEncoder>,
    cancel: &AtomicBool,
    report: &Report,
) -> Result<Option<String>, EngineError> {
    match (settings.format, external) {
        (
            ExportFormat::Video {
                container, audio, ..
            },
            Some(external),
        ) => {
            let format = (container, audio);
            write_external(
                gpu, project, part, format, settings, external, cancel, report,
            )
        }
        (
            ExportFormat::Video {
                container,
                codec,
                audio,
            },
            None,
        ) => write(
            gpu, project, part, container, codec, audio, settings, cancel, report,
        ),
        (ExportFormat::Sound(format), _) => {
            write_sound(project, part, format, settings, cancel, report)
        }
    }
}

/// Writes the whole sequence to `part` as a video file; the encoder used, or `None` when
/// cancelled.
#[allow(clippy::too_many_arguments)]
fn write(
    gpu: &Gpu,
    project: &Arc<Project>,
    part: &Path,
    container: Container,
    codec: VideoCodec,
    audio: AudioCodec,
    settings: &ExportSettings,
    cancel: &AtomicBool,
    report: &Report,
) -> Result<Option<String>, EngineError> {
    let sequence = project.sequence();
    let (rate, end) = (sequence.frame_rate(), sequence.end());
    if end <= Frame(0) {
        return Err(EngineError::Empty);
    }
    let sound = has_sound(project);
    let (width, height) = export_size(sequence.resolution(), settings.short_side);
    let video = VideoSettings {
        codec,
        quality: settings.quality,
        ..VideoSettings::h264(width, height, (rate.num(), rate.den()))
    };
    let audio = sound.then(|| settings.audio(audio));
    let mut writer = Writer::create(part, container, video, audio)?;
    let mut frames = Frames::new(gpu, writer.size());
    let mut mix = sound.then(|| Mix::new(project));
    let total = end.0 as u64;
    let mut reported = Instant::now();
    for frame in 0..end.0 {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        writer.write_video(&frames.draw(project, Frame(frame))?)?;
        if let Some(mix) = mix.as_mut() {
            writer.write_audio(mix.through(Frame(frame))?)?;
        }
        let done = frame as u64 + 1;
        if done == total || reported.elapsed() >= PROGRESS_INTERVAL {
            reported = Instant::now();
            report(EngineEvent::Export(ExportEvent::Progress { done, total }));
        }
    }
    let encoder = writer.encoder().to_owned();
    writer.finish()?;
    Ok(Some(encoder))
}

/// Writes the sequence's sound alone to `part`; the encoder used, or `None` when cancelled.
fn write_sound(
    project: &Arc<Project>,
    part: &Path,
    format: AudioFormat,
    settings: &ExportSettings,
    cancel: &AtomicBool,
    report: &Report,
) -> Result<Option<String>, EngineError> {
    let end = project.sequence().end();
    if end <= Frame(0) {
        return Err(EngineError::Empty);
    }
    let mut writer = Writer::sound(part, format, settings.audio(format.codec()))?;
    let mut mix = Mix::new(project);
    let total = end.0 as u64;
    let mut reported = Instant::now();
    for frame in 0..end.0 {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        writer.write_audio(mix.through(Frame(frame))?)?;
        let done = frame as u64 + 1;
        if done == total || reported.elapsed() >= PROGRESS_INTERVAL {
            reported = Instant::now();
            report(EngineEvent::Export(ExportEvent::Progress { done, total }));
        }
    }
    let encoder = writer.encoder().to_owned();
    writer.finish()?;
    Ok(Some(encoder))
}

/// Writes the whole sequence to `part` as a video file through a user's own `ffmpeg`
/// (docs/ARCHITECTURE.md, "Optional GPL encoders"): the sound is mixed into a WAV file beside
/// the part file first, then every frame is drawn as for Dusk's own encoders and piped in.
/// The encoder used, or `None` when cancelled.
#[allow(clippy::too_many_arguments)]
fn write_external(
    gpu: &Gpu,
    project: &Arc<Project>,
    part: &Path,
    (container, audio): (Container, AudioCodec),
    settings: &ExportSettings,
    external: &ExternalEncoder,
    cancel: &AtomicBool,
    report: &Report,
) -> Result<Option<String>, EngineError> {
    let sequence = project.sequence();
    let (rate, end) = (sequence.frame_rate(), sequence.end());
    if end <= Frame(0) {
        return Err(EngineError::Empty);
    }
    let fps = f64::from(rate.num()) / f64::from(rate.den());
    let wanted = export_size(sequence.resolution(), settings.short_side);
    let size = external.encoder.fit(wanted, fps);
    // The WAV never outlives the export, however it ends.
    let sound = Removed(sound_part(part));
    let mixed = has_sound(project);
    if mixed {
        let mut writer =
            Writer::sound(&sound.0, AudioFormat::Wav, settings.audio(AudioCodec::Pcm))?;
        let mut mix = Mix::new(project);
        for frame in 0..end.0 {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            writer.write_audio(mix.through(Frame(frame))?)?;
        }
        writer.finish()?;
    }
    let job = ExternalJob {
        size,
        rate: (rate.num(), rate.den()),
        sound: mixed.then_some(sound.0.as_path()),
        container,
        quality: settings.quality,
        audio: settings.audio(audio),
        output: part,
    };
    let total = end.0 as u64;
    let progress = {
        let report = Arc::clone(report);
        move |encoded: u64| {
            let done = encoded.min(total);
            report(EngineEvent::Export(ExportEvent::Progress { done, total }));
        }
    };
    // Dropping it on the way out stops the program.
    let mut encoding = Encoding::start(external, &job, progress)?;
    let mut frames = Frames::new(gpu, size);
    for frame in 0..end.0 {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if !encoding.write(frames.draw(project, Frame(frame))?, cancel)? {
            return Ok(None);
        }
    }
    if !encoding.finish(cancel)? {
        return Ok(None);
    }
    Ok(Some(external.label()))
}

/// Where an external export mixes the sound for the program to read: beside the part file,
/// as `name.ext.part.wav`.
pub(crate) fn sound_part(part: &Path) -> PathBuf {
    let mut name = part.as_os_str().to_owned();
    name.push(".wav");
    PathBuf::from(name)
}

/// A file removed when this is dropped, if it is there.
struct Removed(PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Draws the sequence's frames for an encoder: each at the export size, in limited-range
/// BT.709 YUV.
struct Frames {
    compositor: Compositor,
    to_yuv: ToYuv,
    decoders: Decoders,
    size: (u32, u32),
}

impl Frames {
    fn new(gpu: &Gpu, size: (u32, u32)) -> Frames {
        Frames {
            compositor: Compositor::new(gpu),
            to_yuv: ToYuv::new(gpu),
            decoders: Decoders::default(),
            size,
        }
    }

    /// Timeline frame `frame` of `project`, as the encoder takes it.
    fn draw(&mut self, project: &Project, frame: Frame) -> Result<Picture, EngineError> {
        let texture = match self.decoders.picture(project, frame)? {
            Some(picture) => {
                let placement = placement_at(project, frame);
                self.compositor
                    .render_placed(&picture, &placement, self.size)?
            }
            None => self.compositor.blank(self.size)?,
        };
        Ok(self.to_yuv.convert(&texture)?)
    }
}

/// The sequence's sound, mixed a frame at a time.
struct Mix {
    mixer: Mixer,
    rate: Rational,
    samples: Vec<f32>,
    /// Sample frames mixed so far.
    mixed: u64,
}

impl Mix {
    fn new(project: &Arc<Project>) -> Mix {
        Mix {
            mixer: Mixer::new(Arc::clone(project), AUDIO_RATE, MediaTime(0), 1.0),
            rate: project.sequence().frame_rate(),
            samples: Vec::new(),
            mixed: 0,
        }
    }

    /// The sound from where the last call ended to the end of timeline frame `frame`,
    /// interleaved stereo.
    fn through(&mut self, frame: Frame) -> Result<&[f32], EngineError> {
        let until = frame_to_media(Frame(frame.0 + 1), self.rate).0;
        let until = (i128::from(until) * i128::from(AUDIO_RATE) / 1_000_000) as u64;
        self.samples.resize(2 * (until - self.mixed) as usize, 0.0);
        self.mixer.fill(&mut self.samples)?;
        self.mixed = until;
        Ok(&self.samples)
    }
}

/// The export's own decoders, at most two open (docs/ARCHITECTURE.md, "Decoder pool"). An
/// export decodes every frame in order, so each video frame is used once and never cached;
/// the still shown last is kept, since it shows for many frames.
#[derive(Default)]
struct Decoders {
    open: Vec<(MediaId, VideoDecoder)>,
    still: Option<(MediaId, Arc<Picture>)>,
}

impl Decoders {
    /// The picture shown at `frame`, or `None` where it is black.
    fn picture(
        &mut self,
        project: &Project,
        frame: Frame,
    ) -> Result<Option<Arc<Picture>>, EngineError> {
        let sequence = project.sequence();
        let Some(clip) = sequence.visible_video_at(frame) else {
            return Ok(None);
        };
        if let Some(media) = project
            .media_ref(clip.media_id)
            .filter(|media| media.info.kind == MediaKind::Still)
        {
            if let Some((id, picture)) = &self.still
                && *id == media.id
            {
                return Ok(Some(Arc::clone(picture)));
            }
            let size = still_size(&media.info, sequence.resolution());
            let picture = Arc::new(dusk_media::decode_still(&media.path, size)?);
            self.still = Some((media.id, Arc::clone(&picture)));
            return Ok(Some(picture));
        }
        let time = clip.source_time_at(frame, sequence.frame_rate());
        let index = match self
            .open
            .iter()
            .position(|(media, _)| *media == clip.media_id)
        {
            Some(index) => index,
            None => {
                let Some(media) = project.media_ref(clip.media_id) else {
                    return Ok(None);
                };
                if self.open.len() >= 2 {
                    self.open.remove(0);
                }
                let decoder = VideoDecoder::open(&media.path, Acceleration::Software)?;
                self.open.push((clip.media_id, decoder));
                self.open.len() - 1
            }
        };
        let decoded = self.open[index].1.frame_at(time)?;
        Ok(decoded.map(|decoded| Arc::new(decoded.picture)))
    }
}
