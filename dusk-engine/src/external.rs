//! A user's own `ffmpeg` program, for the GPL encoders Dusk does not ship (docs/ARCHITECTURE.md,
//! "Optional GPL encoders"): Dusk draws every frame itself and pipes them in raw, with the
//! sound mixed into a WAV file beside the part file, and reads the program's `-progress` for
//! the progress bar. Nothing GPL is linked or shipped; the program is one the user installed.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{SendTimeoutError, Sender};
use dusk_core::Picture;
use dusk_media::{AudioCodec, AudioSettings, Container, EXTERNAL_ENCODERS, Encoder, VideoCodec};

use crate::EngineError;

/// An encoder in a user's own `ffmpeg` program.
#[derive(Clone, Debug)]
pub struct ExternalEncoder {
    /// The program.
    pub program: PathBuf,
    /// The encoder: one of [`EXTERNAL_ENCODERS`], though any encoder of Dusk's table that the
    /// program has is driven the same way.
    pub encoder: &'static Encoder,
}

impl ExternalEncoder {
    /// How an export names it, such as "libx264 in your ffmpeg".
    pub fn label(&self) -> String {
        format!("{} in your ffmpeg", self.encoder.name)
    }
}

/// How long a program may take to list its encoders.
const LISTING_PATIENCE: Duration = Duration::from_secs(10);

/// The encoders of [`EXTERNAL_ENCODERS`] that the `ffmpeg` program at `program` has; none
/// when it has none. It runs the program, so it belongs off the UI thread.
pub fn external_encoders(program: &Path) -> Result<Vec<ExternalEncoder>, EngineError> {
    let mut child = command(program)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| EngineError::ExternalProgram {
            path: program.to_path_buf(),
            source,
        })?;
    // The listing is longer than a pipe holds, so it is read while the program runs.
    let stdout = child.stdout.take();
    let reader = thread::Builder::new()
        .name("dusk ffmpeg listing".to_owned())
        .spawn(move || {
            let mut listing = Vec::new();
            if let Some(mut stdout) = stdout {
                let _ = stdout.read_to_end(&mut listing);
            }
            String::from_utf8_lossy(&listing).into_owned()
        });
    let deadline = Instant::now() + LISTING_PATIENCE;
    let listed = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let listing = reader
        .map_err(EngineError::Thread)?
        .join()
        .unwrap_or_default();
    let found = found_in(&listing)
        .filter(|_| listed)
        .ok_or_else(|| EngineError::NotFfmpeg {
            path: program.to_path_buf(),
        })?;
    Ok(found
        .into_iter()
        .map(|encoder| ExternalEncoder {
            program: program.to_path_buf(),
            encoder,
        })
        .collect())
}

/// The encoders of [`EXTERNAL_ENCODERS`] that `listing`, what `ffmpeg -encoders` prints,
/// names; `None` when it is not such a listing.
fn found_in(listing: &str) -> Option<Vec<&'static Encoder>> {
    if !listing.lines().any(|line| line.trim() == "Encoders:") {
        return None;
    }
    let found = EXTERNAL_ENCODERS
        .iter()
        .filter(|encoder| {
            listing
                .lines()
                .any(|line| line.split_whitespace().nth(1) == Some(encoder.name))
        })
        .collect();
    Some(found)
}

/// A command that runs `program` without a console window of its own: it is a console
/// program, and Dusk is not.
fn command(program: &Path) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// What an external encoder is asked to make.
pub(crate) struct ExternalJob<'a> {
    /// The frames' size.
    pub size: (u32, u32),
    /// How many frames come a second, as numerator and denominator.
    pub rate: (u32, u32),
    /// The sound, mixed into a WAV file, when there is any.
    pub sound: Option<&'a Path>,
    pub container: Container,
    pub quality: dusk_media::Quality,
    pub audio: AudioSettings,
    /// Where to write: the export's part file.
    pub output: &'a Path,
}

/// The tags of the frames Dusk draws for an encoder: limited-range BT.709 with chroma sited
/// left, as `ffmpeg` names them.
const TAGS: [&str; 10] = [
    "-color_range",
    "tv",
    "-colorspace",
    "bt709",
    "-color_primaries",
    "bt709",
    "-color_trc",
    "bt709",
    "-chroma_sample_location",
    "left",
];

/// `path` as a file URL, which `ffmpeg` never takes for another protocol, whatever the name.
fn file_url(path: &Path) -> OsString {
    let mut url = OsString::from("file:");
    url.push(path);
    url
}

/// The command line that has `encoder` encode raw NV12 frames from its standard input, with
/// the sound from `job.sound`, into `job.output`, reporting how far it is on its standard
/// output.
pub(crate) fn external_args(encoder: &ExternalEncoder, job: &ExternalJob) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let push = |args: &mut Vec<OsString>, items: &[&str]| {
        args.extend(items.iter().map(OsString::from));
    };
    // The part file is the export's own, left over only by a crash.
    push(
        &mut args,
        &["-hide_banner", "-nostats", "-loglevel", "error", "-y"],
    );
    // Its inputs are the pipe and a file, never a URL (docs/REQUIREMENTS.md, "Offline and
    // private").
    let size = format!("{}x{}", job.size.0, job.size.1);
    let rate = format!("{}/{}", job.rate.0, job.rate.1);
    push(
        &mut args,
        &[
            "-protocol_whitelist",
            "pipe",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "nv12",
            "-video_size",
            &size,
            "-framerate",
            &rate,
        ],
    );
    push(&mut args, &TAGS);
    push(&mut args, &["-i", "pipe:0"]);
    if let Some(sound) = job.sound {
        push(
            &mut args,
            &["-protocol_whitelist", "file", "-f", "wav", "-i"],
        );
        args.push(file_url(sound));
        push(&mut args, &["-map", "0:v", "-map", "1:a"]);
    } else {
        push(&mut args, &["-map", "0:v"]);
    }
    push(&mut args, &["-c:v", encoder.encoder.name]);
    // The quality maps as for Dusk's own encoders, from the same table.
    let fps = f64::from(job.rate.0) / f64::from(job.rate.1.max(1));
    for (name, value) in encoder.encoder.command_line(job.quality, job.size, fps) {
        args.push(format!("-{name}").into());
        args.push(value.into());
    }
    push(&mut args, &TAGS);
    let mpeg4 = matches!(job.container, Container::Mp4 | Container::Mov);
    if encoder.encoder.codec == VideoCodec::Hevc && mpeg4 {
        // As Apple's players need (see `dusk-media`'s writer).
        push(&mut args, &["-tag:v", "hvc1"]);
    }
    if job.sound.is_some() {
        let codec = match job.audio.codec {
            AudioCodec::Aac => "aac",
            AudioCodec::Opus => "libopus",
            AudioCodec::Mp3 => "libmp3lame",
            AudioCodec::Pcm => "pcm_s16le",
        };
        let bit_rate = job.audio.bit_rate.to_string();
        push(&mut args, &["-c:a", codec, "-b:a", &bit_rate]);
    }
    if mpeg4 {
        push(&mut args, &["-movflags", "+faststart"]);
    }
    push(
        &mut args,
        &["-progress", "pipe:1", "-f", job.container.muxer()],
    );
    args.push(file_url(job.output));
    args
}

/// How many of the program's last lines of errors are kept, to say why it stopped.
const ERROR_LINES: usize = 4;

/// How often a wait on the program looks at the cancel flag.
const WAIT_STEP: Duration = Duration::from_millis(20);

/// How long a program that stopped taking frames gets to end by itself.
const ENDING_PATIENCE: Duration = Duration::from_secs(2);

/// What became of a frame handed to a [`Feeder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fed {
    /// It is on its way to the program.
    Taken,
    /// The export was cancelled while the program took no frames.
    Cancelled,
    /// The program stopped taking frames.
    Stopped,
}

/// Writes frames into a program's input on a thread of its own, so the export never waits
/// on the program longer than it takes to notice a cancel, even when the program stops
/// reading without ending.
struct Feeder {
    frames: Option<Sender<Picture>>,
    thread: Option<JoinHandle<()>>,
}

impl Feeder {
    /// Starts writing the frames it is fed into `input`, NV12 planes one after another.
    fn start(mut input: impl Write + Send + 'static) -> Result<Feeder, EngineError> {
        // One frame waits while the one before it is written.
        let (frames, waiting) = crossbeam_channel::bounded::<Picture>(1);
        let thread = thread::Builder::new()
            .name("dusk ffmpeg frames".to_owned())
            .spawn(move || {
                for picture in waiting {
                    let written = input
                        .write_all(&picture.luma)
                        .and_then(|()| input.write_all(&picture.chroma));
                    if written.is_err() {
                        // Dropping the receiver tells the export.
                        return;
                    }
                }
                // Dropping the input tells the program the last frame came.
            })
            .map_err(EngineError::Thread)?;
        Ok(Feeder {
            frames: Some(frames),
            thread: Some(thread),
        })
    }

    /// Hands `picture` over, waiting while the program is busy unless `cancel` is set.
    fn feed(&self, picture: Picture, cancel: &AtomicBool) -> Fed {
        let Some(frames) = &self.frames else {
            return Fed::Stopped;
        };
        let mut picture = picture;
        loop {
            match frames.send_timeout(picture, WAIT_STEP) {
                Ok(()) => return Fed::Taken,
                Err(SendTimeoutError::Timeout(back)) if !cancel.load(Ordering::Relaxed) => {
                    picture = back;
                }
                Err(SendTimeoutError::Timeout(_)) => return Fed::Cancelled,
                Err(SendTimeoutError::Disconnected(_)) => return Fed::Stopped,
            }
        }
    }

    /// No more frames: the thread writes the one waiting, if the program takes it, and lets
    /// go of the input.
    fn close(&mut self) {
        self.frames = None;
    }

    /// Closes and waits for the thread, which ends once the program takes the last frame or
    /// is gone.
    fn join(&mut self) {
        self.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A user's `ffmpeg` encoding: frames go in through its standard input, how many it has
/// encoded comes back through its `-progress`. Dropping it before [`finish`](Self::finish)
/// stops the program.
pub(crate) struct Encoding {
    child: Child,
    feeder: Feeder,
    progress: Option<JoinHandle<()>>,
    errors: Option<JoinHandle<Vec<String>>>,
}

impl Encoding {
    /// Starts `encoder`'s program on `job`; `progress` hears how many frames it has encoded,
    /// on a thread of its own, each time that count goes up.
    pub(crate) fn start(
        encoder: &ExternalEncoder,
        job: &ExternalJob,
        progress: impl Fn(u64) + Send + 'static,
    ) -> Result<Encoding, EngineError> {
        let mut child = command(&encoder.program)
            .args(external_args(encoder, job))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| EngineError::ExternalProgram {
                path: encoder.program.clone(),
                source,
            })?;
        // Both are read as the program writes, so neither pipe fills up and stalls it.
        let stdout = child.stdout.take();
        let progress = thread::Builder::new()
            .name("dusk ffmpeg progress".to_owned())
            .spawn(move || {
                let Some(stdout) = stdout else {
                    return;
                };
                let mut encoded = 0;
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    let frames = line
                        .strip_prefix("frame=")
                        .and_then(|count| count.trim().parse::<u64>().ok());
                    if let Some(frames) = frames.filter(|frames| *frames > encoded) {
                        encoded = frames;
                        progress(encoded);
                    }
                }
            });
        let stderr = child.stderr.take();
        let errors = thread::Builder::new()
            .name("dusk ffmpeg errors".to_owned())
            .spawn(move || {
                let mut last = Vec::new();
                let Some(stderr) = stderr else {
                    return last;
                };
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    let line = line.trim();
                    if !line.is_empty() {
                        last.push(line.to_owned());
                        if last.len() > ERROR_LINES {
                            last.remove(0);
                        }
                    }
                }
                last
            });
        let feeder = match child.stdin.take() {
            Some(stdin) => Feeder::start(stdin),
            None => Err(EngineError::ExternalFailed {
                message: "it took no frames".to_owned(),
            }),
        };
        let progress = progress.map_err(EngineError::Thread);
        let errors = errors.map_err(EngineError::Thread);
        match (feeder, progress, errors) {
            (Ok(feeder), Ok(progress), Ok(errors)) => Ok(Encoding {
                child,
                feeder,
                progress: Some(progress),
                errors: Some(errors),
            }),
            (Err(error), ..) | (_, Err(error), _) | (.., Err(error)) => {
                // Its pipes close as it ends, which ends the threads that did start.
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    }

    /// Hands the program an NV12 frame of the job's size: `false` when `cancel` was set while
    /// the program took no frames. When the program stopped taking them, the error is what it
    /// said.
    pub(crate) fn write(
        &mut self,
        picture: Picture,
        cancel: &AtomicBool,
    ) -> Result<bool, EngineError> {
        match self.feeder.feed(picture, cancel) {
            Fed::Taken => Ok(true),
            Fed::Cancelled => Ok(false),
            Fed::Stopped => Err(self.failure()),
        }
    }

    /// Lets the program finish the file: `true` when it ends well, `false` when `cancel` was
    /// set first; else the error is what it said.
    pub(crate) fn finish(mut self, cancel: &AtomicBool) -> Result<bool, EngineError> {
        self.feeder.close();
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) if status.success() => {
                    self.join();
                    return Ok(true);
                }
                Ok(Some(status)) => return Err(failed(self.join(), &status.to_string())),
                // Dropping it stops the program.
                Ok(None) if cancel.load(Ordering::Relaxed) => return Ok(false),
                Ok(None) => thread::sleep(WAIT_STEP),
                Err(error) => return Err(failed(self.join(), &error.to_string())),
            }
        }
    }

    /// Why the program stopped taking frames: it is ending, so this gives it a moment, stops
    /// it if it has not, and reads what it said.
    fn failure(&mut self) -> EngineError {
        self.feeder.close();
        let deadline = Instant::now() + ENDING_PATIENCE;
        let ended = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break status.to_string(),
                Ok(None) if Instant::now() < deadline => thread::sleep(WAIT_STEP),
                Ok(None) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break "it stopped taking frames".to_owned();
                }
                Err(error) => break error.to_string(),
            }
        };
        failed(self.join(), &ended)
    }

    /// Waits for the threads, which end with the program, and returns its last lines of
    /// errors.
    fn join(&mut self) -> Vec<String> {
        self.feeder.join();
        if let Some(progress) = self.progress.take() {
            let _ = progress.join();
        }
        self.errors
            .take()
            .and_then(|errors| errors.join().ok())
            .unwrap_or_default()
    }
}

impl Drop for Encoding {
    /// Stops the program if it is still running, so a cancelled or failed export never
    /// leaves it writing.
    fn drop(&mut self) {
        self.feeder.close();
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.join();
    }
}

/// The error of a program that ended as `ended` says, having said `errors` last.
fn failed(errors: Vec<String>, ended: &str) -> EngineError {
    let message = if errors.is_empty() {
        format!("it ended with {ended}")
    } else {
        errors.join(" ")
    };
    EngineError::ExternalFailed { message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use dusk_core::color::{Primaries, Transfer};
    use dusk_core::{ChromaSiting, ColorMatrix, ColorRange, PictureLayout};
    use dusk_media::{Quality, encoder_named, external_encoder_named};

    /// A 4x2 NV12 picture whose luma is all `value` and chroma all `value + 1`.
    fn picture(value: u8) -> Picture {
        Picture {
            width: 4,
            height: 2,
            layout: PictureLayout::Nv12,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            primaries: Primaries::Bt709,
            transfer: Transfer::Bt1886,
            peak_nits: 0,
            siting: ChromaSiting::LEFT,
            luma: vec![value; 8],
            chroma: vec![value + 1; 4],
        }
    }

    /// A program's input that keeps what it is given.
    #[derive(Clone, Default)]
    struct Kept(Arc<Mutex<Vec<u8>>>);

    impl Write for Kept {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn frames_reach_the_program_whole_and_in_order() {
        let kept = Kept::default();
        let mut feeder = Feeder::start(kept.clone()).unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(feeder.feed(picture(10), &cancel), Fed::Taken);
        assert_eq!(feeder.feed(picture(20), &cancel), Fed::Taken);
        feeder.join();
        let mut expected = vec![10; 8];
        expected.extend([11; 4]);
        expected.extend([20; 8]);
        expected.extend([21; 4]);
        assert_eq!(*kept.0.lock().unwrap(), expected);
    }

    /// A program's input that takes nothing until it is let go.
    struct Stuck(Arc<AtomicBool>);

    impl Write for Stuck {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            while !self.0.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(5));
            }
            Err(std::io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_cancel_is_heard_while_the_program_takes_no_frames() {
        let let_go = Arc::new(AtomicBool::new(false));
        let mut feeder = Feeder::start(Stuck(Arc::clone(&let_go))).unwrap();
        let cancel = AtomicBool::new(false);
        // One frame is being written and one waits; the next cannot be handed over.
        assert_eq!(feeder.feed(picture(1), &cancel), Fed::Taken);
        assert_eq!(feeder.feed(picture(2), &cancel), Fed::Taken);
        cancel.store(true, Ordering::Relaxed);
        let asked = Instant::now();
        assert_eq!(feeder.feed(picture(3), &cancel), Fed::Cancelled);
        assert!(asked.elapsed() < Duration::from_secs(1));
        let_go.store(true, Ordering::Relaxed);
        feeder.join();
    }

    /// A program's input that is closed.
    struct Closed;

    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_program_that_stops_taking_frames_is_noticed() {
        let mut feeder = Feeder::start(Closed).unwrap();
        let cancel = AtomicBool::new(false);
        let fed: Vec<Fed> = (0..4).map(|n| feeder.feed(picture(n), &cancel)).collect();
        assert!(fed.contains(&Fed::Stopped), "{fed:?}");
        feeder.join();
    }

    const LISTING: &str = "Encoders:
 V..... = Video
 ------
 V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 (codec h264)
 V....D libx264rgb           libx264 H.264 / AVC / MPEG-4 AVC / MPEG-4 part 10 RGB (codec h264)
 V..... libopenh264          OpenH264 Cisco H.264 encoder (codec h264)
 A....D aac                  AAC (Advanced Audio Coding)
";

    fn names(listing: &str) -> Option<Vec<&'static str>> {
        found_in(listing).map(|found| found.iter().map(|encoder| encoder.name).collect())
    }

    #[test]
    fn the_listing_says_which_gpl_encoders_the_program_has() {
        assert_eq!(names(LISTING), Some(vec!["libx264"]));
        let both =
            format!("{LISTING} V....D libx265              libx265 H.265 / HEVC (codec hevc)\n");
        assert_eq!(names(&both), Some(vec!["libx264", "libx265"]));
        let without = "Encoders:\n V..... libopenh264          OpenH264\n";
        assert_eq!(names(without), Some(vec![]));
    }

    #[test]
    fn a_program_that_lists_no_encoders_is_not_ffmpeg() {
        assert_eq!(names("Microsoft Windows [Version 10.0.26200]\n"), None);
        assert_eq!(names(""), None);
        // A listing names the encoder in its second column, not anywhere in a line.
        assert_eq!(names("Encoders:\nlibx264 is not here\n"), Some(vec![]));
    }

    fn line(args: &[OsString]) -> String {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn x265() -> ExternalEncoder {
        ExternalEncoder {
            program: "ffmpeg.exe".into(),
            encoder: external_encoder_named("libx265").unwrap(),
        }
    }

    fn job<'a>(sound: Option<&'a Path>, quality: Quality) -> ExternalJob<'a> {
        ExternalJob {
            size: (1920, 1080),
            rate: (30000, 1001),
            sound,
            container: Container::Mp4,
            quality,
            audio: AudioSettings::of(AudioCodec::Aac, 48_000),
            output: Path::new("out.mp4.part"),
        }
    }

    #[test]
    fn the_program_gets_raw_frames_tagged_as_dusk_draws_them() {
        let args = line(&external_args(
            &x265(),
            &job(Some(Path::new("mix.wav")), Quality::HIGH),
        ));
        // It writes over a part file a crash left behind, which it could not ask about.
        assert!(
            args.starts_with("-hide_banner -nostats -loglevel error -y "),
            "{args}"
        );
        assert!(
            args.contains(
                "-protocol_whitelist pipe -f rawvideo -pix_fmt nv12 -video_size 1920x1080 \
                 -framerate 30000/1001 -color_range tv -colorspace bt709 -color_primaries bt709 \
                 -color_trc bt709 -chroma_sample_location left -i pipe:0"
            ),
            "{args}"
        );
        assert!(
            args.contains("-protocol_whitelist file -f wav -i file:mix.wav -map 0:v -map 1:a"),
            "{args}"
        );
        // The quality comes from the encoder table: 21 at High, as for every HEVC encoder.
        assert!(
            args.contains(
                "-c:v libx265 -pix_fmt yuv420p -crf 21 -color_range tv -colorspace bt709"
            ),
            "{args}"
        );
        assert!(args.contains("-tag:v hvc1"), "{args}");
        assert!(args.contains("-c:a aac -b:a 192000"), "{args}");
        assert!(
            args.ends_with("-movflags +faststart -progress pipe:1 -f mp4 file:out.mp4.part"),
            "{args}"
        );
    }

    #[test]
    fn without_sound_the_program_reads_the_frames_alone() {
        let args = line(&external_args(
            &x265(),
            &job(None, Quality::Bitrate(4_000_000)),
        ));
        assert!(
            args.contains("-b:v 4000000 -maxrate 4800000 -bufsize 4800000"),
            "{args}"
        );
        assert!(
            !args.contains("wav") && !args.contains("1:a") && !args.contains("-c:a"),
            "{args}"
        );
        assert!(args.contains("-map 0:v -c:v libx265"), "{args}");
    }

    #[test]
    fn h264_in_matroska_takes_no_mp4_flags() {
        let x264 = ExternalEncoder {
            program: "ffmpeg.exe".into(),
            encoder: external_encoder_named("libx264").unwrap(),
        };
        let job = ExternalJob {
            container: Container::Mkv,
            ..job(None, Quality::Crf(30))
        };
        let args = line(&external_args(&x264, &job));
        assert!(
            args.contains("-c:v libx264 -pix_fmt yuv420p -crf 30"),
            "{args}"
        );
        assert!(
            !args.contains("hvc1") && !args.contains("faststart"),
            "{args}"
        );
        assert!(args.ends_with("-f matroska file:out.mp4.part"), "{args}");
    }

    #[test]
    fn the_encoder_is_named_after_the_program() {
        let openh264 = ExternalEncoder {
            program: "ffmpeg.exe".into(),
            encoder: encoder_named("libopenh264").unwrap(),
        };
        assert_eq!(openh264.label(), "libopenh264 in your ffmpeg");
    }

    /// The pinned build's own `ffmpeg` program, when it is there.
    fn own_ffmpeg() -> Option<PathBuf> {
        let program = PathBuf::from(std::env::var_os("FFMPEG_DIR")?)
            .join("bin")
            .join(if cfg!(windows) {
                "ffmpeg.exe"
            } else {
                "ffmpeg"
            });
        program.is_file().then_some(program)
    }

    #[test]
    fn the_pinned_builds_own_ffmpeg_has_no_gpl_encoders() {
        let Some(program) = own_ffmpeg() else {
            eprintln!("skipped: no ffmpeg program in the pinned build");
            return;
        };
        assert!(external_encoders(&program).unwrap().is_empty());
    }

    #[test]
    fn a_program_that_is_not_there_is_reported() {
        let missing = Path::new("no such folder/ffmpeg.exe");
        assert!(matches!(
            external_encoders(missing),
            Err(EngineError::ExternalProgram { .. })
        ));
    }
}
