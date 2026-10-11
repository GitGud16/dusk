//! Dusk's helper (docs/ARCHITECTURE.md, "Export"): a video export is encoded in `dusq`,
//! started for that export, so that what a hardware encoder keeps after its session closes
//! goes back to the system when the helper ends. Dusk writes messages to the helper's
//! standard input and reads lines from its standard output; this module holds both sides,
//! the helper's ([`serve_helper`], built into `dusq`) and Dusk's (with the `gpu` feature).

use std::io::{Read, Write};
use std::path::PathBuf;

use dusk_core::color::{Primaries, Transfer};
use dusk_core::{ChromaSiting, ColorMatrix, ColorRange, Picture, PictureLayout};
use dusk_media::{
    AudioCodec, AudioSettings, Container, Quality, VideoCodec, VideoSettings, Writer,
    available_encoders,
};

/// The `dusq` command that serves Dusk as its helper; `dusq --help` does not list it.
pub const HELPER_COMMAND: &str = "encode-for-dusk";

/// What every line the helper says to Dusk starts with. Other lines on its standard output,
/// such as the settings Kvazaar and SVT-AV1 print, are not meant for Dusk.
const SAYS: &str = "dusk-helper";

/// The kinds of message Dusk sends: a kind byte, a little-endian 32-bit length, that many
/// bytes.
mod kind {
    /// List the encoders that open; nothing follows.
    pub const PROBE: u8 = b'P';
    /// The part file to write, as the system's own path bytes; the job follows.
    pub const OUTPUT: u8 = b'O';
    /// What to make, as [`HelperJob`](super::HelperJob) text.
    pub const JOB: u8 = b'J';
    /// A frame: NV12 at the size the helper said it is ready for, luma then chroma.
    pub const FRAME: u8 = b'V';
    /// Sound mixed through the frame before it: interleaved stereo 32-bit floats.
    pub const SOUND: u8 = b'A';
    /// The last frame came: finish the file. Input that ends without it means Dusk is gone,
    /// and the unfinished file is removed.
    pub const END: u8 = b'E';
}

/// Writes one message of `kind` made of `parts`, one after another.
#[cfg(any(feature = "gpu", test))]
fn write_message(output: &mut impl Write, kind: u8, parts: &[&[u8]]) -> std::io::Result<()> {
    let length: usize = parts.iter().map(|part| part.len()).sum();
    let length = u32::try_from(length)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "too long"))?;
    output.write_all(&[kind])?;
    output.write_all(&length.to_le_bytes())?;
    for part in parts {
        output.write_all(part)?;
    }
    Ok(())
}

/// Reads the next message: its kind and bytes, or `None` where the input ends between two.
fn read_message(input: &mut impl Read) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let mut head = [0; 5];
    let mut got = 0;
    while got < head.len() {
        match input.read(&mut head[got..])? {
            0 if got == 0 => return Ok(None),
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            read => got += read,
        }
    }
    let length = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    Ok(Some((head[0], bytes)))
}

/// `path` as the system's own bytes: UTF-16 on Windows, so no name is changed on the way.
#[cfg(any(feature = "gpu", test))]
fn path_bytes(path: &std::path::Path) -> Vec<u8> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
}

/// The path [`path_bytes`] made.
fn path_from(bytes: &[u8]) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        let (pairs, []) = bytes.as_chunks::<2>() else {
            return None;
        };
        let wide: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        Some(std::ffi::OsString::from_wide(&wide).into())
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(std::ffi::OsStr::from_bytes(bytes).into())
    }
}

/// What a helper is asked to make: an export's video, and its sound when there is any.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HelperJob {
    pub container: Container,
    pub codec: VideoCodec,
    /// The wanted size; the encoder may need it smaller.
    pub size: (u32, u32),
    /// Frames a second, as numerator and denominator.
    pub rate: (u32, u32),
    pub quality: Quality,
    pub audio: Option<AudioSettings>,
}

impl HelperJob {
    /// The job as `name=value` lines.
    #[cfg(any(feature = "gpu", test))]
    fn to_text(self) -> String {
        let quality = match self.quality {
            Quality::Level(level) => format!("level:{level}"),
            Quality::Bitrate(bits) => format!("bitrate:{bits}"),
            Quality::Crf(crf) => format!("crf:{crf}"),
        };
        let mut text = format!(
            "container={}\ncodec={}\nsize={}x{}\nrate={}/{}\nquality={quality}\n",
            self.container.extension(),
            codec_token(self.codec),
            self.size.0,
            self.size.1,
            self.rate.0,
            self.rate.1,
        );
        if let Some(audio) = self.audio {
            text.push_str(&format!(
                "audio={}:{}:{}\n",
                audio_token(audio.codec),
                audio.rate,
                audio.bit_rate
            ));
        }
        text
    }

    /// The job [`to_text`](Self::to_text) wrote; `None` for text it did not write.
    fn parse(text: &str) -> Option<HelperJob> {
        let value = |name: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
        };
        let container = Container::ALL
            .into_iter()
            .find(|container| value("container") == Some(container.extension()))?;
        let codec = [
            VideoCodec::H264,
            VideoCodec::Hevc,
            VideoCodec::Av1,
            VideoCodec::Vp9,
        ]
        .into_iter()
        .find(|codec| value("codec") == Some(codec_token(*codec)))?;
        let (width, height) = value("size")?.split_once('x')?;
        let (num, den) = value("rate")?.split_once('/')?;
        let quality = match value("quality")?.split_once(':')? {
            ("level", level) => Quality::Level(level.parse().ok()?),
            ("bitrate", bits) => Quality::Bitrate(bits.parse().ok()?),
            ("crf", crf) => Quality::Crf(crf.parse().ok()?),
            _ => return None,
        };
        let audio = match value("audio") {
            Some(audio) => {
                let mut parts = audio.split(':');
                let codec = [
                    AudioCodec::Aac,
                    AudioCodec::Opus,
                    AudioCodec::Mp3,
                    AudioCodec::Pcm,
                ]
                .into_iter()
                .find(|codec| parts.next() == Some(audio_token(*codec)))?;
                Some(AudioSettings {
                    codec,
                    rate: parts.next()?.parse().ok()?,
                    bit_rate: parts.next()?.parse().ok()?,
                })
            }
            None => None,
        };
        Some(HelperJob {
            container,
            codec,
            size: (width.parse().ok()?, height.parse().ok()?),
            rate: (num.parse().ok()?, den.parse().ok()?),
            quality,
            audio,
        })
    }

    fn video(self) -> VideoSettings {
        VideoSettings {
            codec: self.codec,
            quality: self.quality,
            ..VideoSettings::h264(self.size.0, self.size.1, self.rate)
        }
    }
}

fn codec_token(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "h264",
        VideoCodec::Hevc => "hevc",
        VideoCodec::Av1 => "av1",
        VideoCodec::Vp9 => "vp9",
    }
}

fn audio_token(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Aac => "aac",
        AudioCodec::Opus => "opus",
        AudioCodec::Mp3 => "mp3",
        AudioCodec::Pcm => "pcm",
    }
}

/// Says `line` to Dusk.
fn say(output: &mut impl Write, line: &str) -> std::io::Result<()> {
    writeln!(output, "{SAYS} {line}")?;
    output.flush()
}

/// Serves Dusk as its helper, reading its messages from `input` and answering on `output`:
/// lists the encoders that open, or writes one export's file and finishes it. `false` when
/// something failed, which it has said.
pub fn serve_helper(mut input: impl Read, mut output: impl Write) -> bool {
    match serve(&mut input, &mut output) {
        Ok(()) => true,
        Err(message) => {
            let _ = say(&mut output, &format!("error {message}"));
            false
        }
    }
}

fn serve(input: &mut impl Read, output: &mut impl Write) -> Result<(), String> {
    let lost = |error: std::io::Error| format!("Dusk's messages were cut off ({error})");
    let unexpected = || "Dusk sent a message the helper does not know".to_owned();
    let Some((first, bytes)) = read_message(input).map_err(lost)? else {
        return Err("Dusk sent nothing".to_owned());
    };
    if first == kind::PROBE {
        for encoder in available_encoders() {
            say(output, &format!("encoder {}", encoder.name)).map_err(lost)?;
        }
        return say(output, "done").map_err(lost);
    }
    if first != kind::OUTPUT {
        return Err(unexpected());
    }
    let part = path_from(&bytes).ok_or_else(unexpected)?;
    let job = match read_message(input).map_err(lost)? {
        Some((kind::JOB, bytes)) => std::str::from_utf8(&bytes)
            .ok()
            .and_then(HelperJob::parse)
            .ok_or_else(unexpected)?,
        _ => return Err(unexpected()),
    };
    let mut writer = Writer::create(&part, job.container, job.video(), job.audio)
        .map_err(|error| error.to_string())?;
    let (width, height) = writer.size();
    let ready = format!("ready {width} {height} {}", writer.encoder());
    say(output, &ready).map_err(lost)?;
    let written = write_all(input, &mut writer, (width, height))
        .and_then(|()| writer.finish().map_err(|error| error.to_string()));
    if let Err(error) = written {
        // Dusk removes it too, when it is still there to.
        let _ = std::fs::remove_file(&part);
        return Err(error);
    }
    say(output, "done").map_err(lost)
}

/// Encodes the frames and sound Dusk sends, up to its end message.
fn write_all(
    input: &mut impl Read,
    writer: &mut Writer,
    (width, height): (u32, u32),
) -> Result<(), String> {
    let lost = |error: std::io::Error| format!("Dusk's messages were cut off ({error})");
    let unexpected = || "Dusk sent a message the helper does not know".to_owned();
    let luma = width as usize * height as usize;
    loop {
        match read_message(input).map_err(lost)? {
            Some((kind::FRAME, bytes)) if bytes.len() == luma + luma / 2 => {
                let mut luma_bytes = bytes;
                let chroma = luma_bytes.split_off(luma);
                writer
                    .write_video(&nv12(width, height, luma_bytes, chroma))
                    .map_err(|error| error.to_string())?;
            }
            Some((kind::SOUND, bytes)) if bytes.len().is_multiple_of(4) => {
                let samples: Vec<f32> = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|four| f32::from_le_bytes(*four))
                    .collect();
                writer
                    .write_audio(&samples)
                    .map_err(|error| error.to_string())?;
            }
            Some((kind::END, _)) => return Ok(()),
            Some(_) => return Err(unexpected()),
            None => {
                return Err(
                    "Dusk stopped before the end of the export, so its unfinished file is \
                     removed; export again"
                        .to_owned(),
                );
            }
        }
    }
}

/// An NV12 frame as Dusk draws them for an encoder: limited-range BT.709, chroma sited left.
fn nv12(width: u32, height: u32, luma: Vec<u8>, chroma: Vec<u8>) -> Picture {
    Picture {
        width,
        height,
        layout: PictureLayout::Nv12,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        primaries: Primaries::Bt709,
        transfer: Transfer::Bt1886,
        peak_nits: 0,
        siting: ChromaSiting::LEFT,
        luma,
        chroma,
    }
}

#[cfg(feature = "gpu")]
pub use client::{Helper, probe_encoders};
#[cfg(feature = "gpu")]
pub(crate) use client::{HelperEncoding, Started};

/// Dusk's side: starting the helper, feeding it and hearing what it says.
#[cfg(feature = "gpu")]
mod client {
    use std::ffi::OsString;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use crossbeam_channel::{Receiver, RecvTimeoutError};
    use dusk_core::Picture;
    use dusk_media::{Encoder, available_encoders, encoder_named};

    use super::{HELPER_COMMAND, HelperJob, SAYS, kind, path_bytes, write_message};
    use crate::EngineError;
    use crate::pipe::{Fed, Feeder, WAIT_STEP, command};

    /// The program that encodes Dusk's video exports: `dusq` beside `dusk.exe`.
    #[derive(Clone, Debug)]
    pub struct Helper {
        /// The program.
        pub program: PathBuf,
        /// What comes before the messages on its command line.
        pub args: Vec<OsString>,
    }

    impl Helper {
        /// `dusq` at `program`, serving Dusk.
        pub fn dusq(program: PathBuf) -> Helper {
            Helper {
                program,
                args: vec![HELPER_COMMAND.into()],
            }
        }

        /// `dusq` beside the running program, when it is there.
        pub fn beside_this_program() -> Option<Helper> {
            let name = if cfg!(windows) { "dusq.exe" } else { "dusq" };
            let program = std::env::current_exe().ok()?.with_file_name(name);
            program.is_file().then(|| Helper::dusq(program))
        }
    }

    /// What Dusk sends the helper.
    enum Message {
        Probe,
        Output(Vec<u8>),
        Job(String),
        Frame(Picture),
        Sound(Vec<f32>),
        End,
    }

    fn write_item(input: &mut impl Write, message: &Message) -> std::io::Result<()> {
        match message {
            Message::Probe => write_message(input, kind::PROBE, &[]),
            Message::Output(path) => write_message(input, kind::OUTPUT, &[path]),
            Message::Job(text) => write_message(input, kind::JOB, &[text.as_bytes()]),
            Message::Frame(picture) => {
                write_message(input, kind::FRAME, &[&picture.luma, &picture.chroma])
            }
            Message::Sound(samples) => {
                let bytes: Vec<u8> = samples
                    .iter()
                    .flat_map(|sample| sample.to_le_bytes())
                    .collect();
                write_message(input, kind::SOUND, &[&bytes])
            }
            Message::End => write_message(input, kind::END, &[]),
        }
    }

    /// What the helper said.
    #[derive(Debug, PartialEq, Eq)]
    enum Said {
        Ready((u32, u32), String),
        Encoder(String),
        Done,
        Error(String),
    }

    /// The helper's line `line`, when it is one meant for Dusk. What a test harness or an
    /// encoder wrote without ending its line may come before it, so it is found wherever
    /// it starts.
    fn heard(line: &str) -> Option<Said> {
        let marker = format!("{SAYS} ");
        let said = &line[line.find(&marker)? + marker.len()..];
        let (word, rest) = said.split_once(' ').unwrap_or((said, ""));
        match word {
            "ready" => {
                let mut parts = rest.splitn(3, ' ');
                let width = parts.next()?.parse().ok()?;
                let height = parts.next()?.parse().ok()?;
                Some(Said::Ready((width, height), parts.next()?.to_owned()))
            }
            "encoder" => Some(Said::Encoder(rest.to_owned())),
            "done" => Some(Said::Done),
            "error" => Some(Said::Error(rest.to_owned())),
            _ => None,
        }
    }

    /// How many of the helper's last lines of errors are kept, to say why it stopped.
    const ERROR_LINES: usize = 4;

    /// How long a helper that stopped taking input gets to end by itself.
    const ENDING_PATIENCE: Duration = Duration::from_secs(2);

    /// How long a probe may take.
    const PROBE_PATIENCE: Duration = Duration::from_secs(30);

    /// How long an encoder may take to open; hardware encoders take a second or two.
    const READY_PATIENCE: Duration = Duration::from_secs(60);

    /// A helper running: messages go in through its standard input, what it says comes back
    /// on a channel. Dropping it before [`finish`](Self::finish) stops it.
    pub(crate) struct HelperEncoding {
        child: Child,
        feeder: Feeder<Message>,
        said: Receiver<Said>,
        reader: Option<JoinHandle<()>>,
        errors: Option<JoinHandle<Vec<String>>>,
        program: PathBuf,
    }

    /// What starting an export in the helper came to.
    pub(crate) enum Started {
        /// Ready for frames of `size`, which `encoder` encodes.
        Ready {
            encoding: Box<HelperEncoding>,
            size: (u32, u32),
            encoder: String,
        },
        /// Cancelled before the helper was ready.
        Cancelled,
    }

    impl HelperEncoding {
        /// Starts `helper` and hands it its first messages.
        fn spawn(helper: &Helper, first: Vec<Message>) -> Result<HelperEncoding, EngineError> {
            let failed_to_start = |source| EngineError::HelperStart {
                path: helper.program.clone(),
                source,
            };
            let mut child = command(&helper.program)
                .args(&helper.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(failed_to_start)?;
            let (tell, said) = crossbeam_channel::unbounded();
            // Both are read as the helper writes, so neither pipe fills up and stalls it.
            let stdout = child.stdout.take();
            let reader = thread::Builder::new()
                .name("dusk helper output".to_owned())
                .spawn(move || {
                    let Some(stdout) = stdout else {
                        return;
                    };
                    for line in BufReader::new(stdout).lines() {
                        let Ok(line) = line else {
                            break;
                        };
                        if let Some(said) = heard(line.trim_end()) {
                            let _ = tell.send(said);
                        }
                    }
                });
            let stderr = child.stderr.take();
            let errors = thread::Builder::new()
                .name("dusk helper errors".to_owned())
                .spawn(move || last_lines(stderr));
            let feeder = match child.stdin.take() {
                Some(stdin) => Feeder::start(stdin, write_item),
                None => Err(EngineError::HelperFailed {
                    message: "Dusk could not write to its encoder, dusq".to_owned(),
                }),
            };
            match (feeder, reader, errors) {
                (Ok(feeder), Ok(reader), Ok(errors)) => {
                    let encoding = HelperEncoding {
                        child,
                        feeder,
                        said,
                        reader: Some(reader),
                        errors: Some(errors),
                        program: helper.program.clone(),
                    };
                    let never = AtomicBool::new(false);
                    for message in first {
                        if encoding.feeder.feed(message, &never) != Fed::Taken {
                            let mut encoding = encoding;
                            return Err(encoding.failure());
                        }
                    }
                    Ok(encoding)
                }
                (Err(error), ..) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    Err(error)
                }
                (_, Err(error), _) | (.., Err(error)) => {
                    // Its pipes close as it ends, which ends the threads that did start.
                    let _ = child.kill();
                    let _ = child.wait();
                    Err(EngineError::Thread(error))
                }
            }
        }

        /// Starts `helper` writing `part` as `job` asks, and waits until its encoder is open.
        pub(crate) fn start(
            helper: &Helper,
            part: &Path,
            job: HelperJob,
            cancel: &AtomicBool,
        ) -> Result<Started, EngineError> {
            let first = vec![
                Message::Output(path_bytes(part)),
                Message::Job(job.to_text()),
            ];
            let mut encoding = HelperEncoding::spawn(helper, first)?;
            let deadline = Instant::now() + READY_PATIENCE;
            loop {
                match encoding.said.recv_timeout(WAIT_STEP) {
                    Ok(Said::Ready(size, encoder)) => {
                        return Ok(Started::Ready {
                            encoding: Box::new(encoding),
                            size,
                            encoder,
                        });
                    }
                    Ok(Said::Error(message)) => return Err(EngineError::HelperFailed { message }),
                    Ok(_) => return Err(encoding.failure()),
                    Err(RecvTimeoutError::Timeout) if cancel.load(Ordering::Relaxed) => {
                        return Ok(Started::Cancelled);
                    }
                    Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                    Err(_) => return Err(encoding.failure()),
                }
            }
        }

        /// Hands the helper an NV12 frame of the size it is ready for: `false` when `cancel`
        /// was set while it took nothing.
        pub(crate) fn frame(
            &mut self,
            picture: Picture,
            cancel: &AtomicBool,
        ) -> Result<bool, EngineError> {
            self.feed(Message::Frame(picture), cancel)
        }

        /// Hands the helper the sound mixed through the frame before: interleaved stereo.
        pub(crate) fn sound(
            &mut self,
            samples: Vec<f32>,
            cancel: &AtomicBool,
        ) -> Result<bool, EngineError> {
            self.feed(Message::Sound(samples), cancel)
        }

        fn feed(&mut self, message: Message, cancel: &AtomicBool) -> Result<bool, EngineError> {
            match self.feeder.feed(message, cancel) {
                Fed::Taken => Ok(true),
                Fed::Cancelled => Ok(false),
                Fed::Stopped => Err(self.failure()),
            }
        }

        /// Lets the helper finish the file: `true` once it says it has, `false` when `cancel`
        /// was set first.
        pub(crate) fn finish(mut self, cancel: &AtomicBool) -> Result<bool, EngineError> {
            if !self.feed(Message::End, cancel)? {
                return Ok(false);
            }
            self.feeder.close();
            loop {
                match self.said.recv_timeout(WAIT_STEP) {
                    Ok(Said::Done) => break,
                    Ok(Said::Error(message)) => return Err(EngineError::HelperFailed { message }),
                    Ok(_) => {}
                    Err(RecvTimeoutError::Timeout) if cancel.load(Ordering::Relaxed) => {
                        return Ok(false);
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return Err(self.failure()),
                }
            }
            // The file is finished and closed; the helper ends by itself, and one that hangs
            // on the way out is stopped (dropping it waits for that), the file kept.
            let deadline = Instant::now() + ENDING_PATIENCE;
            while Instant::now() < deadline && matches!(self.child.try_wait(), Ok(None)) {
                thread::sleep(WAIT_STEP);
            }
            Ok(true)
        }

        /// Why the helper stopped taking messages or ended early: it is ending, so this gives
        /// it a moment, stops it if it has not, and reads what it said.
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
            // What it said before ending comes first: its own error.
            let errors = self.join();
            if let Some(message) = self.said.try_iter().find_map(|said| match said {
                Said::Error(message) => Some(message),
                _ => None,
            }) {
                return EngineError::HelperFailed { message };
            }
            let name = self
                .program
                .file_name()
                .map_or_else(|| "dusq".into(), |name| name.to_string_lossy());
            let why = if errors.is_empty() {
                ended
            } else {
                format!("{ended}: {}", errors.join(" "))
            };
            EngineError::HelperFailed {
                message: format!(
                    "Dusk's encoder, {name}, stopped ({why}). Export again; if it stops again, \
                     reinstall Dusk"
                ),
            }
        }

        /// Waits for the threads, which end with the helper, and returns its last lines of
        /// errors.
        fn join(&mut self) -> Vec<String> {
            self.feeder.join();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
            self.errors
                .take()
                .and_then(|errors| errors.join().ok())
                .unwrap_or_default()
        }
    }

    impl Drop for HelperEncoding {
        /// Stops the helper if it is still running, so a cancelled or failed export never
        /// leaves it writing, and waits for it to let go of the part file.
        fn drop(&mut self) {
            self.feeder.close();
            if !matches!(self.child.try_wait(), Ok(Some(_))) {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
            self.join();
        }
    }

    /// The last [`ERROR_LINES`] lines `stderr` holds that are not blank.
    fn last_lines(stderr: Option<impl Read>) -> Vec<String> {
        let mut last = Vec::new();
        let Some(stderr) = stderr else {
            return last;
        };
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else {
                break;
            };
            let line = line.trim();
            // SVT-AV1 prints its settings there as it opens.
            if !line.is_empty() && !line.contains("[info]") {
                last.push(line.to_owned());
                if last.len() > ERROR_LINES {
                    last.remove(0);
                }
            }
        }
        last
    }

    /// The encoders that open, as [`available_encoders`] lists them, found by `helper` when
    /// there is one, so that what a hardware encoder keeps after its session closes is the
    /// helper's and goes when it ends; in this process without one, or when the helper
    /// cannot say. It opens real encoding sessions, so it belongs off the UI thread.
    pub fn probe_encoders(helper: Option<&Helper>) -> Vec<&'static Encoder> {
        if let Some(helper) = helper {
            match probe_in(helper) {
                Ok(found) => return found,
                // A development build's stale dusq, say; exports through it say what is wrong.
                Err(error) => eprintln!(
                    "Dusk's encoder probe in {} failed ({error}); probing here instead",
                    helper.program.display()
                ),
            }
        }
        available_encoders().to_vec()
    }

    fn probe_in(helper: &Helper) -> Result<Vec<&'static Encoder>, EngineError> {
        let mut encoding = HelperEncoding::spawn(helper, vec![Message::Probe])?;
        encoding.feeder.close();
        let deadline = Instant::now() + PROBE_PATIENCE;
        let mut found = Vec::new();
        loop {
            match encoding.said.recv_timeout(WAIT_STEP) {
                Ok(Said::Encoder(name)) => found.extend(encoder_named(&name)),
                Ok(Said::Done) => return Ok(found),
                Ok(Said::Error(message)) => return Err(EngineError::HelperFailed { message }),
                Ok(Said::Ready(..)) => return Err(encoding.failure()),
                Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                Err(_) => return Err(encoding.failure()),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn dusk_hears_only_the_lines_meant_for_it() {
            assert_eq!(
                heard("dusk-helper ready 1920 1080 h264_amf"),
                Some(Said::Ready((1920, 1080), "h264_amf".to_owned()))
            );
            assert_eq!(heard("dusk-helper done"), Some(Said::Done));
            // What came before it on the line, as a test harness or an encoder may leave.
            assert_eq!(
                heard("test serve_as_the_helper ... dusk-helper ready 64 48 libopenh264"),
                Some(Said::Ready((64, 48), "libopenh264".to_owned()))
            );
            assert_eq!(
                heard("dusk-helper error no encoder for H.264 could be opened"),
                Some(Said::Error(
                    "no encoder for H.264 could be opened".to_owned()
                ))
            );
            assert_eq!(
                heard("dusk-helper encoder libopenh264"),
                Some(Said::Encoder("libopenh264".to_owned()))
            );
            // What encoders print themselves, and lines that only look like the helper's.
            assert_eq!(heard("Svt[info]: SVT [config]: preset : 8"), None);
            assert_eq!(heard("dusk-helperready 1 1 x"), None);
            assert_eq!(heard("dusk-helper ready wide 1080 x"), None);
            assert_eq!(heard("dusk-helper hello"), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::Path;

    fn job(quality: Quality, audio: Option<AudioSettings>) -> HelperJob {
        HelperJob {
            container: Container::Mkv,
            codec: VideoCodec::Hevc,
            size: (1920, 1080),
            rate: (30000, 1001),
            quality,
            audio,
        }
    }

    #[test]
    fn a_job_reads_back_as_written() {
        let aac = AudioSettings::of(AudioCodec::Aac, 48_000);
        for quality in [Quality::HIGH, Quality::Bitrate(4_000_000), Quality::Crf(30)] {
            for audio in [Some(aac), None] {
                let text = job(quality, audio).to_text();
                let back = HelperJob::parse(&text).expect("it reads back");
                assert_eq!(back.to_text(), text);
                assert_eq!(back.quality, quality);
                assert_eq!(back.audio.is_some(), audio.is_some());
            }
        }
        let text = job(Quality::HIGH, Some(aac)).to_text();
        assert_eq!(
            text,
            "container=mkv\ncodec=hevc\nsize=1920x1080\nrate=30000/1001\nquality=level:80\n\
             audio=aac:48000:192000\n"
        );
    }

    #[test]
    fn a_job_that_dusk_did_not_write_is_refused() {
        let good = job(Quality::HIGH, None).to_text();
        for bad in [
            good.replace("mkv", "avi"),
            good.replace("hevc", "mpeg2"),
            good.replace("1920x1080", "1920"),
            good.replace("level:80", "level:loud"),
            good.replace("rate=30000/1001\n", ""),
            String::new(),
        ] {
            assert!(HelperJob::parse(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn messages_read_back_whole_and_the_end_between_them_is_noticed() {
        let mut bytes = Vec::new();
        write_message(&mut bytes, kind::FRAME, &[&[1, 2, 3], &[4, 5]]).unwrap();
        write_message(&mut bytes, kind::PROBE, &[]).unwrap();
        let mut input = Cursor::new(bytes.clone());
        assert_eq!(
            read_message(&mut input).unwrap(),
            Some((kind::FRAME, vec![1, 2, 3, 4, 5]))
        );
        assert_eq!(
            read_message(&mut input).unwrap(),
            Some((kind::PROBE, vec![]))
        );
        assert_eq!(read_message(&mut input).unwrap(), None);
        // Cut off in the middle of a message.
        let mut cut = Cursor::new(bytes[..7].to_vec());
        assert!(read_message(&mut cut).is_err());
    }

    #[test]
    fn a_path_goes_over_the_pipe_unchanged() {
        let path = Path::new("C:/Users/ünïcødé/clip edit.mp4.part");
        assert_eq!(path_from(&path_bytes(path)).as_deref(), Some(path));
    }

    /// Serves a three-frame 640x480 H.264 export into a part file of its own, with or without
    /// the end message; whether it went well, what it said, and whether the file is there.
    fn served(name: &str, end: bool) -> (bool, String, bool) {
        let dir = std::env::temp_dir().join("dusk-helper-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join(name);
        let _ = std::fs::remove_file(&part);
        let job = HelperJob {
            container: Container::Mp4,
            codec: VideoCodec::H264,
            size: (640, 480),
            rate: (30, 1),
            quality: Quality::HIGH,
            audio: None,
        };
        let mut input = Vec::new();
        write_message(&mut input, kind::OUTPUT, &[&path_bytes(&part)]).unwrap();
        write_message(&mut input, kind::JOB, &[job.to_text().as_bytes()]).unwrap();
        let (luma, chroma) = (vec![16; 640 * 480], vec![128; 640 * 480 / 2]);
        for _ in 0..3 {
            write_message(&mut input, kind::FRAME, &[&luma, &chroma]).unwrap();
        }
        if end {
            write_message(&mut input, kind::END, &[]).unwrap();
        }
        let mut said = Vec::new();
        let served = serve_helper(Cursor::new(input), &mut said);
        let exists = part.exists();
        let _ = std::fs::remove_file(&part);
        (served, String::from_utf8(said).unwrap(), exists)
    }

    #[test]
    fn the_end_message_finishes_the_file_and_without_it_the_file_goes() {
        let (finished, said, exists) = served("ended.mp4.part", true);
        assert!(finished, "{said}");
        assert!(said.ends_with("dusk-helper done\n"), "{said}");
        assert!(exists);
        // Input that stops without it: Dusk was closed or crashed mid-export.
        let (finished, said, exists) = served("cut.mp4.part", false);
        assert!(!finished);
        assert!(
            said.contains("dusk-helper error Dusk stopped before the end of the export"),
            "{said}"
        );
        assert!(!exists);
    }

    #[test]
    fn the_helper_says_what_went_wrong_in_its_own_words() {
        let mut said = Vec::new();
        assert!(!serve_helper(Cursor::new(Vec::new()), &mut said));
        assert_eq!(
            String::from_utf8(said).unwrap(),
            "dusk-helper error Dusk sent nothing\n"
        );
        let mut bytes = Vec::new();
        write_message(&mut bytes, kind::FRAME, &[&[0]]).unwrap();
        let mut said = Vec::new();
        assert!(!serve_helper(Cursor::new(bytes), &mut said));
        assert!(
            String::from_utf8(said)
                .unwrap()
                .starts_with("dusk-helper error Dusk sent a message the helper does not know"),
        );
    }
}
