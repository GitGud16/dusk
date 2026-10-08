//! dusq's CPU transcode path (docs/ARCHITECTURE.md, "Compress tool paths"): one file in and
//! one out, without a graphics adapter, a frame cache or a project. A decoding thread
//! normalizes each frame at the output size and turns it into upright SDR (color steps 1 to 4
//! and the orientation); a queue of four frames carries them to the calling thread, which
//! encodes them and the sound beside them. The file is written as `name.ext.part` and renamed
//! once it is complete, so a cancelled or failed transcode leaves nothing where the user
//! expects a file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use dusk_core::color::SdrConverter;
use dusk_core::{MediaTime, Orientation, SdrPicture, YuvPicture};
use dusk_media::{
    AudioDecoder, AudioFormat, AudioSettings, Container, HUGE_FRAME, LARGE_FRAME, MediaError,
    Quality, StreamDetail, StreamKind, Timing, VideoDecoder, VideoSettings, Writer, probe,
};

use crate::EngineError;
use crate::settings::{AUDIO_RATE, ExportFormat, ExportSettings, export_size};
use crate::size::{corrected, plan_for_size, refused};

/// What dusq's transcode path makes of a file.
#[derive(Clone, Copy, Debug, Default)]
pub struct TranscodeSettings {
    /// The file format and codecs, the picture's short side, the quality and the sound's
    /// bitrate, as an export has them; sound alone takes the sound out.
    pub export: ExportSettings,
    /// Only this encoder, instead of the codec's encoders in order.
    pub encoder: Option<&'static str>,
    /// A constant frame rate, as numerator and denominator; `None` passes the source's own
    /// frame times through, a variable frame rate included.
    pub fps: Option<(u32, u32)>,
    /// Decoder threads; `None` for dusq's default for the source's size
    /// ([`decoder_threads`]).
    pub threads: Option<usize>,
}

/// How far a transcode is: the time reached in the source, of its whole length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    /// The source time written so far.
    pub done: MediaTime,
    /// The source's length.
    pub total: MediaTime,
    /// Which pass: 1, or 2 when a file made to a size came out too far over it and is made
    /// again ([`transcode_to_size`]).
    pub pass: u32,
}

/// A file a transcode wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transcoded {
    /// Where it is.
    pub path: PathBuf,
    /// The encoder that wrote the video, such as `h264_amf`; for sound alone, the sound's.
    pub encoder: String,
    /// The picture's size; (0, 0) for sound alone.
    pub size: (u32, u32),
    /// The file's size in bytes.
    pub bytes: u64,
}

/// Frames that wait between decoding and encoding (docs/REQUIREMENTS.md, "dusq").
const QUEUE: usize = 4;

/// Progress is reported at most this often.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Stereo frames of sound read and written at a time: a tenth of a second.
const SOUND_CHUNK: usize = AUDIO_RATE as usize / 10;

/// dusq's decoder threads for a `width` by `height` source (docs/REQUIREMENTS.md, "dusq"):
/// half the cores, at most 4, for the 1080p class; 2 above it; 1 above 9 Mpx.
pub fn decoder_threads(width: u32, height: u32) -> usize {
    let pixels = u64::from(width) * u64::from(height);
    if pixels > HUGE_FRAME {
        1
    } else if pixels > LARGE_FRAME {
        2
    } else {
        let cores = std::thread::available_parallelism().map_or(2, |cores| cores.get());
        (cores / 2).clamp(1, 4)
    }
}

/// Transcodes `input` into `output` with `settings`, checking `cancel` between frames and
/// calling `progress` as it goes. `None` when cancelled; either way nothing half-written
/// stays behind.
pub fn transcode(
    input: &Path,
    output: &Path,
    settings: &TranscodeSettings,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Option<Transcoded>, EngineError> {
    let part = part_path(output);
    let written = write(input, &part, settings, cancel, progress);
    finish(&part, output, written)
}

/// Transcodes `input` into a video file at `output` of about `target` bytes
/// (docs/ARCHITECTURE.md, "Target file size"): [`plan_for_size`] sets the bitrates, and the
/// ladder the short side, never above the one `settings` asks for. A file that comes out more
/// than 3% over is made again once, smaller, its progress saying pass 2; after that it is kept
/// as it came out. A target below the smallest the video can become is refused with that
/// size. As [`transcode`] otherwise; sound alone is not made to a size.
pub fn transcode_to_size(
    input: &Path,
    output: &Path,
    target: u64,
    settings: &TranscodeSettings,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Option<Transcoded>, EngineError> {
    if matches!(settings.export.format, ExportFormat::Sound(_)) {
        return transcode(input, output, settings, cancel, progress);
    }
    let source = Source::of(input)?;
    let plan = plan_for_size(target, source.length, source.has_sound)
        .map_err(|refusal| refused(refusal, input))?;
    let mut planned = *settings;
    planned.export.quality = Quality::Bitrate(plan.video_bit_rate);
    planned.export.audio_bit_rate = (plan.audio_bit_rate > 0).then_some(plan.audio_bit_rate);
    planned.export.short_side = Some(match settings.export.short_side {
        Some(side) => side.min(plan.short_side),
        None => plan.short_side,
    });
    let part = part_path(output);
    let mut written = write(input, &part, &planned, cancel, progress);
    if written.as_ref().is_ok_and(Option::is_some) {
        let bytes = std::fs::metadata(&part).map_or(0, |file| file.len());
        if let Some(bit_rate) = corrected(plan.video_bit_rate, target, bytes) {
            planned.export.quality = Quality::Bitrate(bit_rate);
            let mut second = |report: Progress| progress(Progress { pass: 2, ..report });
            written = write(input, &part, &planned, cancel, &mut second);
        }
    }
    finish(&part, output, written)
}

/// Writes `input` to `part` as `settings` say; `None` when cancelled.
fn write(
    input: &Path,
    part: &Path,
    settings: &TranscodeSettings,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Option<Encoded>, EngineError> {
    match settings.export.format {
        ExportFormat::Video {
            container,
            codec,
            audio,
        } => {
            let video = VideoSettings {
                codec,
                quality: settings.export.quality,
                encoder: settings.encoder,
                ..VideoSettings::h264(0, 0, (30, 1))
            };
            let audio = settings.export.audio(audio);
            write_video(
                input, part, container, video, audio, settings, cancel, progress,
            )
        }
        ExportFormat::Sound(format) => {
            write_sound(input, part, format, &settings.export, cancel, progress)
        }
    }
}

/// Renames the finished `part` to `output`, or removes what was written.
fn finish(
    part: &Path,
    output: &Path,
    written: Result<Option<Encoded>, EngineError>,
) -> Result<Option<Transcoded>, EngineError> {
    match written {
        Ok(Some(Encoded { encoder, size })) => {
            if let Err(source) = std::fs::rename(part, output) {
                let _ = std::fs::remove_file(part);
                return Err(EngineError::Io {
                    path: output.to_path_buf(),
                    source,
                });
            }
            let bytes = std::fs::metadata(output).map_or(0, |file| file.len());
            Ok(Some(Transcoded {
                path: output.to_path_buf(),
                encoder,
                size,
                bytes,
            }))
        }
        other => {
            // The file may not even have been created.
            let _ = std::fs::remove_file(part);
            other.map(|_| None)
        }
    }
}

/// Takes the sound of `input` out into an `output` file of `format`, at the format's usual
/// bitrate; as [`transcode`] otherwise.
pub fn extract_audio(
    input: &Path,
    output: &Path,
    format: AudioFormat,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Option<Transcoded>, EngineError> {
    let settings = TranscodeSettings {
        export: ExportSettings {
            format: ExportFormat::Sound(format),
            ..ExportSettings::default()
        },
        ..TranscodeSettings::default()
    };
    transcode(input, output, &settings, cancel, progress)
}

/// `path` with `.part` appended: `clip.mp4` becomes `clip.mp4.part`.
pub(crate) fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// What a transcode needs to know of its source before decoding.
struct Source {
    size: (u32, u32),
    orientation: Orientation,
    /// The average frame rate, which the encoder's rate control goes by.
    frame_rate: (u32, u32),
    length: MediaTime,
    has_sound: bool,
}

impl Source {
    fn of(input: &Path) -> Result<Source, EngineError> {
        let info = probe(input)?;
        let video = info.streams.iter().find_map(|stream| match stream.detail {
            StreamDetail::Video {
                width,
                height,
                frame_rate,
                base_frame_rate,
                cover_art: false,
                orientation,
                ..
            } => Some((width, height, frame_rate.or(base_frame_rate), orientation)),
            _ => None,
        });
        let Some((width, height, rate, orientation)) = video else {
            return Err(MediaError::NoVideo {
                path: input.to_path_buf(),
            }
            .into());
        };
        let frame_rate = rate
            .and_then(|(num, den)| Some((u32::try_from(num).ok()?, u32::try_from(den).ok()?)))
            .filter(|(num, den)| *num > 0 && *den > 0)
            .unwrap_or((30, 1));
        Ok(Source {
            size: (width, height),
            orientation,
            frame_rate,
            length: MediaTime(info.duration_us.unwrap_or(0)),
            has_sound: info
                .streams
                .iter()
                .any(|stream| stream.kind == StreamKind::Audio),
        })
    }
}

/// What a job wrote to its part file: the encoder, and the picture's size, (0, 0) for sound
/// alone.
struct Encoded {
    encoder: String,
    size: (u32, u32),
}

/// A frame on its way to the encoder: when it starts in the source, and its picture.
type Frame = Result<(MediaTime, SdrPicture), EngineError>;

/// Writes `input` as a `container` file to `part` with the codec, quality and encoder of
/// `video` and the sound of `audio`; `None` when cancelled.
#[allow(clippy::too_many_arguments)]
fn write_video(
    input: &Path,
    part: &Path,
    container: Container,
    video: VideoSettings,
    audio: AudioSettings,
    settings: &TranscodeSettings,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Option<Encoded>, EngineError> {
    let source = Source::of(input)?;
    let upright = source.orientation.apply_to_size(source.size);
    let (width, height) = export_size(upright, settings.export.short_side);
    let video = VideoSettings {
        width,
        height,
        frame_rate: settings.fps.unwrap_or(source.frame_rate),
        timing: match settings.fps {
            Some(_) => Timing::Constant,
            None => Timing::Source,
        },
        ..video
    };
    let mut sound = if source.has_sound {
        Some(AudioDecoder::open(input, AUDIO_RATE, 2)?)
    } else {
        None
    };
    let audio = sound.is_some().then_some(audio);
    let mut writer = Writer::create(part, container, video, audio)?;
    let size = writer.size();
    // Frames are normalized before they are turned upright, so at the encoder's size turned
    // back.
    let normalized = source.orientation.apply_to_size(size);
    let threads = settings
        .threads
        .unwrap_or_else(|| decoder_threads(source.size.0, source.size.1));
    let (num, den) = source.frame_rate;
    let source_frame = MediaTime((1_000_000 * i64::from(den) / i64::from(num.max(1))).max(1));
    let finished = {
        let mut encode = Encode {
            writer: &mut writer,
            sound: sound.as_mut(),
            sound_written: 0,
            total: source.length,
            reported: None,
            progress: &mut *progress,
        };
        let finished = std::thread::scope(|scope| {
            let (sender, frames) = sync_channel(QUEUE);
            let orientation = source.orientation;
            let decoding = std::thread::Builder::new()
                .name("dusq decode".to_owned())
                .spawn_scoped(scope, move || {
                    decode(input, threads, normalized, orientation, cancel, &sender);
                })
                .map_err(EngineError::Thread)?;
            let encoded = match settings.fps {
                Some(rate) => encode.at_rate(&frames, rate, source_frame, cancel),
                None => encode.as_they_come(&frames, cancel),
            };
            // Hanging up stops the decoding thread if it is still going.
            drop(frames);
            if let Err(panic) = decoding.join() {
                std::panic::resume_unwind(panic);
            }
            encoded
        })?;
        if finished && !cancel.load(Ordering::Relaxed) {
            encode.rest_of_the_sound()?;
        }
        finished
    };
    if !finished || cancel.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let encoder = writer.encoder().to_owned();
    writer.finish()?;
    progress(Progress {
        done: source.length,
        total: source.length,
        pass: 1,
    });
    Ok(Some(Encoded { encoder, size }))
}

/// The decoding thread: frames of `input`, normalized at `size`, upright and in SDR, into
/// `frames` until the stream ends, the encoder hangs up or `cancel` is set.
fn decode(
    input: &Path,
    threads: usize,
    size: (u32, u32),
    orientation: Orientation,
    cancel: &AtomicBool,
    frames: &SyncSender<Frame>,
) {
    let mut decoder = match VideoDecoder::open_with_threads(input, threads) {
        Ok(decoder) => decoder,
        Err(error) => {
            let _ = frames.send(Err(error.into()));
            return;
        }
    };
    let workers =
        (std::thread::available_parallelism().map_or(2, |cores| cores.get()) / 2).clamp(1, 4);
    // Step 3 for the tags of the frames, made again if they change.
    let mut step_3 = None;
    while !cancel.load(Ordering::Relaxed) {
        let frame = match decoder.next_normalized(size) {
            Ok(Some(frame)) => frame,
            Ok(None) => return,
            Err(error) => {
                let _ = frames.send(Err(error.into()));
                return;
            }
        };
        let picture = &frame.picture;
        let tags = (picture.primaries, picture.transfer, picture.peak_nits);
        if step_3
            .as_ref()
            .is_none_or(|(made_for, _)| *made_for != tags)
        {
            step_3 = Some((tags, SdrConverter::new(tags.0, tags.1, f64::from(tags.2))));
        }
        let converter = step_3
            .as_ref()
            .and_then(|(_, converter)| converter.as_ref());
        let sdr = upright(picture, converter, orientation, workers);
        if frames.send(Ok((frame.time, sdr))).is_err() {
            return;
        }
    }
}

/// `picture` through color steps 2 to 4 and `orientation`, in bands of rows on up to
/// `workers` threads.
pub(crate) fn upright(
    picture: &YuvPicture,
    converter: Option<&SdrConverter>,
    orientation: Orientation,
    workers: usize,
) -> SdrPicture {
    let (width, height) = orientation.apply_to_size((picture.width, picture.height));
    let mut planes = [0; 3].map(|_| vec![0u8; width as usize * height as usize]);
    let rows = height.div_ceil(workers.max(1) as u32).max(1);
    let band = rows as usize * width as usize;
    if band > 0 {
        let [red, green, blue] = &mut planes;
        std::thread::scope(|scope| {
            let bands = red
                .chunks_mut(band)
                .zip(green.chunks_mut(band))
                .zip(blue.chunks_mut(band));
            for (index, ((red, green), blue)) in bands.enumerate() {
                let first_row = index as u32 * rows;
                scope.spawn(move || {
                    picture.sdr_rows(converter, orientation, first_row, [red, green, blue]);
                });
            }
        });
    }
    SdrPicture {
        width,
        height,
        planes,
    }
}

/// The encoding side of a video transcode: frames into the writer, with the sound beside
/// them.
struct Encode<'a> {
    writer: &'a mut Writer,
    sound: Option<&'a mut AudioDecoder>,
    /// Stereo frames of sound written so far.
    sound_written: u64,
    total: MediaTime,
    reported: Option<Instant>,
    progress: &'a mut dyn FnMut(Progress),
}

impl Encode<'_> {
    /// Encodes each frame at its own time; false when stopped before the end.
    fn as_they_come(
        &mut self,
        frames: &Receiver<Frame>,
        cancel: &AtomicBool,
    ) -> Result<bool, EngineError> {
        for frame in frames {
            if cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let (time, picture) = frame?;
            self.write(&picture, time)?;
        }
        Ok(!cancel.load(Ordering::Relaxed))
    }

    /// Encodes a frame every `1/rate` seconds, each showing the last source frame that has
    /// started by then (the first before it starts), until the last source frame has been
    /// shown for `source_frame`, a frame at the source's average rate; false when stopped
    /// before the end.
    fn at_rate(
        &mut self,
        frames: &Receiver<Frame>,
        (num, den): (u32, u32),
        source_frame: MediaTime,
        cancel: &AtomicBool,
    ) -> Result<bool, EngineError> {
        let tick = |n: i64| {
            let micros = i128::from(n) * 1_000_000 * i128::from(den) / i128::from(num.max(1));
            MediaTime(i64::try_from(micros).unwrap_or(i64::MAX))
        };
        let mut next = frames.recv().ok().transpose()?;
        let mut shown: Option<(MediaTime, SdrPicture)> = None;
        for n in 0.. {
            if cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let time = tick(n);
            while next.as_ref().is_some_and(|(start, _)| *start <= time) {
                shown = next.take();
                next = frames.recv().ok().transpose()?;
            }
            let Some((start, picture)) = shown.as_ref().or(next.as_ref()) else {
                break;
            };
            if next.is_none() && time.0 >= start.0 + source_frame.0 {
                break;
            }
            self.write(picture, time)?;
        }
        Ok(!cancel.load(Ordering::Relaxed))
    }

    /// Writes the sound up to `time`, then the frame that starts there.
    fn write(&mut self, picture: &SdrPicture, time: MediaTime) -> Result<(), EngineError> {
        self.sound_until(time)?;
        self.writer.write_sdr(picture, time)?;
        let due = self
            .reported
            .is_none_or(|reported| reported.elapsed() >= PROGRESS_INTERVAL);
        if due {
            self.reported = Some(Instant::now());
            (self.progress)(Progress {
                done: time.min(self.total),
                total: self.total,
                pass: 1,
            });
        }
        Ok(())
    }

    /// Writes the source's sound up to `time`, or as far as it goes.
    fn sound_until(&mut self, time: MediaTime) -> Result<(), EngineError> {
        let until = (i128::from(time.0.max(0)) * i128::from(AUDIO_RATE) / 1_000_000) as u64;
        self.sound_while(|written| written < until)
    }

    /// Writes what is left of the source's sound.
    fn rest_of_the_sound(&mut self) -> Result<(), EngineError> {
        self.sound_while(|_| true)
    }

    /// Writes sound while `more` says so of the frames written, until the sound ends.
    fn sound_while(&mut self, more: impl Fn(u64) -> bool) -> Result<(), EngineError> {
        let Some(sound) = self.sound.as_deref_mut() else {
            return Ok(());
        };
        let mut samples = vec![0.0; 2 * SOUND_CHUNK];
        while more(self.sound_written) {
            let read = sound.read(&mut samples)?;
            if read == 0 {
                break;
            }
            self.writer.write_audio(&samples[..2 * read])?;
            self.sound_written += read as u64;
        }
        Ok(())
    }
}

/// Writes the sound of `input` alone to `part` in `format`; `None` when cancelled.
fn write_sound(
    input: &Path,
    part: &Path,
    format: AudioFormat,
    export: &ExportSettings,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Option<Encoded>, EngineError> {
    let mut sound = AudioDecoder::open(input, AUDIO_RATE, 2)?;
    let total = MediaTime(probe(input)?.duration_us.unwrap_or(0));
    let mut writer = Writer::sound(part, format, export.audio(format.codec()))?;
    let mut samples = vec![0.0; 2 * SOUND_CHUNK];
    let mut written = 0u64;
    let mut reported: Option<Instant> = None;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let read = sound.read(&mut samples)?;
        if read == 0 {
            break;
        }
        writer.write_audio(&samples[..2 * read])?;
        written += read as u64;
        if reported.is_none_or(|reported| reported.elapsed() >= PROGRESS_INTERVAL) {
            reported = Some(Instant::now());
            let done = MediaTime((i128::from(written) * 1_000_000 / i128::from(AUDIO_RATE)) as i64);
            progress(Progress {
                done: done.min(total),
                total,
                pass: 1,
            });
        }
    }
    let encoder = writer.encoder().to_owned();
    writer.finish()?;
    progress(Progress {
        done: total,
        total,
        pass: 1,
    });
    Ok(Some(Encoded {
        encoder,
        size: (0, 0),
    }))
}
