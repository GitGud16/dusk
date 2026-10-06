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
use dusk_core::{Frame, MediaId, MediaKind, MediaTime, Picture, Project};
use dusk_media::{
    Acceleration, AudioCodec, AudioFormat, Container, VideoCodec, VideoDecoder, VideoSettings,
    Writer,
};
use dusk_render::{Compositor, Gpu, ToYuv};

use crate::EngineError;
use crate::engine::{EngineEvent, Report};
use crate::info::still_size;
use crate::mixer::Mixer;
use crate::placement::placement_at;
use crate::settings::{AUDIO_RATE, ExportFormat, ExportSettings, export_size, has_sound};
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
    /// The file is complete.
    Finished {
        /// Where it is.
        path: PathBuf,
        /// The video encoder that wrote it, such as `h264_amf`.
        encoder: String,
    },
    /// The export was cancelled; nothing was left behind.
    Cancelled,
    /// The export failed; nothing was left behind.
    Failed(EngineError),
}

/// A running export.
#[derive(Clone)]
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

/// Starts exporting `project` to `path` on a new thread, which ends after reporting the
/// outcome.
pub(crate) fn spawn(
    gpu: &Gpu,
    project: Arc<Project>,
    path: PathBuf,
    settings: ExportSettings,
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
                let outcome = export(&gpu, &project, &path, &settings, &cancel, &report);
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

/// Exports, then renames the finished file or removes what was written.
fn export(
    gpu: &Gpu,
    project: &Arc<Project>,
    path: &Path,
    settings: &ExportSettings,
    cancel: &AtomicBool,
    report: &Report,
) -> ExportEvent {
    let part = part_path(path);
    let outcome = match settings.format {
        ExportFormat::Video {
            container,
            codec,
            audio,
        } => write(
            gpu, project, &part, container, codec, audio, settings, cancel, report,
        ),
        ExportFormat::Sound(format) => {
            write_sound(project, &part, format, settings, cancel, report)
        }
    };
    let renamed = match outcome {
        Ok(Some(encoder)) => match std::fs::rename(&part, path) {
            Ok(()) => {
                return ExportEvent::Finished {
                    path: path.to_path_buf(),
                    encoder,
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
        Err(error) => ExportEvent::Failed(error),
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
    let size = writer.size();
    let compositor = Compositor::new(gpu);
    let to_yuv = ToYuv::new(gpu);
    let mut decoders = Decoders::default();
    let mut mixer = sound.then(|| Mixer::new(Arc::clone(project), AUDIO_RATE, MediaTime(0), 1.0));
    let mut samples = Vec::new();
    let mut mixed = 0u64;
    let total = end.0 as u64;
    let mut reported = Instant::now();
    for frame in 0..end.0 {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let texture = match decoders.picture(project, Frame(frame))? {
            Some(picture) => {
                let placement = placement_at(project, Frame(frame));
                compositor.render_placed(&picture, &placement, size)?
            }
            None => compositor.blank(size)?,
        };
        writer.write_video(&to_yuv.convert(&texture)?)?;
        if let Some(mixer) = mixer.as_mut() {
            // The sound up to the end of this frame.
            let until = frame_to_media(Frame(frame + 1), rate).0;
            let until = (i128::from(until) * i128::from(AUDIO_RATE) / 1_000_000) as u64;
            samples.resize(2 * (until - mixed) as usize, 0.0);
            mixer.fill(&mut samples)?;
            writer.write_audio(&samples)?;
            mixed = until;
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
    let sequence = project.sequence();
    let (rate, end) = (sequence.frame_rate(), sequence.end());
    if end <= Frame(0) {
        return Err(EngineError::Empty);
    }
    let mut writer = Writer::sound(part, format, settings.audio(format.codec()))?;
    let mut mixer = Mixer::new(Arc::clone(project), AUDIO_RATE, MediaTime(0), 1.0);
    let mut samples = Vec::new();
    let mut mixed = 0u64;
    let total = end.0 as u64;
    let mut reported = Instant::now();
    for frame in 0..end.0 {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let until = frame_to_media(Frame(frame + 1), rate).0;
        let until = (i128::from(until) * i128::from(AUDIO_RATE) / 1_000_000) as u64;
        samples.resize(2 * (until - mixed) as usize, 0.0);
        mixer.fill(&mut samples)?;
        writer.write_audio(&samples)?;
        mixed = until;
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
