//! dusq's command line: which command, on which file, with which options.

use std::path::PathBuf;

use dusk_engine::{AudioFormat, Container, Quality, VideoCodec};

/// What dusq was asked to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Show how to use dusq.
    Help,
    /// Show dusq's version.
    Version,
    /// Compress a video into a smaller file.
    Compress(Compress),
    /// Take the sound of a file out into a file of its own.
    ExtractAudio(Extract),
}

/// `dusq compress`.
#[derive(Clone, Debug, PartialEq)]
pub struct Compress {
    pub input: PathBuf,
    /// Where to write; `None` for a name beside the input.
    pub output: Option<PathBuf>,
    pub container: Container,
    pub codec: VideoCodec,
    pub quality: Quality,
    /// A target file size in bytes, instead of a quality: the bitrates and the picture size
    /// follow from it.
    pub size: Option<u64>,
    /// The picture's short side; `None` keeps the source's.
    pub short_side: Option<u32>,
    /// A constant frame rate, as numerator and denominator.
    pub fps: Option<(u32, u32)>,
    /// Decoder threads.
    pub threads: Option<usize>,
    /// Replace the output file if it exists.
    pub overwrite: bool,
}

/// `dusq extract-audio`.
#[derive(Clone, Debug, PartialEq)]
pub struct Extract {
    pub input: PathBuf,
    /// Where to write; `None` for a name beside the input.
    pub output: Option<PathBuf>,
    pub format: AudioFormat,
    /// Replace the output file if it exists.
    pub overwrite: bool,
}

/// How to use dusq, laid out in columns: a raw string, so what it holds is what dusq prints.
pub const USAGE: &str = r#"dusq compresses videos and takes their sound out, without opening Dusk.

Usage:
  dusq compress <video> [options]
  dusq extract-audio <file> [options]

Options for compress:
  -o, --output <file>    where to write; by default "<name> compressed.mp4" beside the video
      --quality <q>      high, medium, small, or 0 to 100 (default: medium)
      --size <size>      a file size to aim at instead, such as 25MB, 800KB or 1.5GB; the
                         picture size and bitrate follow from it
      --short-side <px>  the picture's short side, such as 1080, 720 or 480; never enlarges
                         (default: the video's own size)
      --format <f>       mp4, mov, mkv or webm (default: mp4, or the output's extension)
      --codec <c>        h264, hevc, av1 or vp9 (default: h264, or vp9 for webm)
      --fps <rate>       a constant frame rate, such as 30 or 30000/1001 (default: the
                         video's own frame times)
      --threads <n>      decoder threads; more decode faster and take more memory
      --overwrite        replace the output file if it exists

Options for extract-audio:
  -o, --output <file>    where to write; by default "<name>.mp3" beside the file
      --format <f>       mp3, aac, opus or wav (default: mp3, or the output's extension)
      --overwrite        replace the output file if it exists

      --                 ends the options, for a file whose name starts with -
  -h, --help             show this
  -V, --version          show dusq's version
"#;

/// Reads the command line, `args` without the program's name; a message saying what is
/// wrong when it cannot.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let Some(command) = args.first() else {
        return Ok(Command::Help);
    };
    let compressing = match command.as_str() {
        "-h" | "--help" | "help" => return Ok(Command::Help),
        "-V" | "--version" => return Ok(Command::Version),
        "compress" => true,
        "extract-audio" => false,
        other => {
            return Err(format!(
                "dusq has no command \"{other}\"; it has compress and extract-audio (see dusq --help)"
            ));
        }
    };
    let mut options = Options::default();
    let mut rest = args[1..].iter();
    // After "--" every argument is a file, so a name may start with a dash.
    let mut files_only = false;
    while let Some(arg) = rest.next() {
        if files_only || arg == "--" {
            if files_only {
                take_file(&mut options, arg)?;
            }
            files_only = true;
            continue;
        }
        // Long options may carry their value after an equals sign.
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        // An option without a value takes none: "--overwrite=no" is not an overwrite.
        let alone = || match &inline {
            Some(_) => Err(format!(
                "{name} takes no value; leave out what follows the ="
            )),
            None => Ok(()),
        };
        let mut value = |example: &str| match inline.clone().or_else(|| rest.next().cloned()) {
            Some(value) => Ok(value),
            None => Err(format!("{name} needs a value, such as {example}")),
        };
        match name {
            "-h" | "--help" => return Ok(Command::Help),
            "-o" | "--output" => options.output = Some(PathBuf::from(value("out.mp4")?)),
            "--format" => options.format = Some(value("mp4")?),
            "--overwrite" => {
                alone()?;
                options.overwrite = true;
            }
            "--quality" if compressing => options.quality = Some(value("medium")?),
            "--size" if compressing => options.size = Some(value("25MB")?),
            "--short-side" if compressing => options.short_side = Some(value("720")?),
            "--codec" if compressing => options.codec = Some(value("h264")?),
            "--fps" if compressing => options.fps = Some(value("30")?),
            "--threads" if compressing => options.threads = Some(value("2")?),
            option if option.starts_with('-') && option.len() > 1 => {
                let command = if compressing {
                    "compress"
                } else {
                    "extract-audio"
                };
                return Err(format!(
                    "{command} has no option {option} (see dusq --help)"
                ));
            }
            file => take_file(&mut options, file)?,
        }
    }
    if compressing {
        options.compress().map(Command::Compress)
    } else {
        options.extract().map(Command::ExtractAudio)
    }
}

/// `file` is the input, which dusq takes one of.
fn take_file(options: &mut Options, file: &str) -> Result<(), String> {
    if options.input.is_some() {
        return Err(format!(
            "dusq takes one file at a time, and {file} is one too many"
        ));
    }
    options.input = Some(PathBuf::from(file));
    Ok(())
}

/// The options as given, before they are checked.
#[derive(Default)]
struct Options {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    format: Option<String>,
    quality: Option<String>,
    size: Option<String>,
    short_side: Option<String>,
    codec: Option<String>,
    fps: Option<String>,
    threads: Option<String>,
    overwrite: bool,
}

impl Options {
    fn compress(self) -> Result<Compress, String> {
        let input = self
            .input
            .ok_or("compress needs a video: dusq compress <video>")?;
        let named = match self.output.as_deref().and_then(extension) {
            Some(extension) => Some(container_named(&extension).ok_or_else(|| {
                format!(
                    "dusq cannot write .{extension} files; name the output .mp4, .mov, .mkv or .webm"
                )
            })?),
            None => None,
        };
        let container = match self.format.as_deref() {
            Some(format) => {
                let container = container_named(&format.to_lowercase())
                    .ok_or_else(|| format!("--format takes mp4, mov, mkv or webm, not {format}"))?;
                if named.is_some_and(|named| named != container) {
                    return Err(format!(
                        "the output is named for another format than --format {format}; make them agree"
                    ));
                }
                container
            }
            None => named.unwrap_or(Container::Mp4),
        };
        let codec = match self.codec.as_deref() {
            Some(codec) => {
                let codec = codec_named(&codec.to_lowercase())
                    .ok_or_else(|| format!("--codec takes h264, hevc, av1 or vp9, not {codec}"))?;
                if !container.video_codecs().contains(&codec) {
                    return Err(format!(
                        "{} files cannot hold {}; choose another --codec or --format",
                        container.name(),
                        codec.name()
                    ));
                }
                codec
            }
            None => container.video_codecs()[0],
        };
        if self.quality.is_some() && self.size.is_some() {
            return Err("--quality and --size do not go together; choose one".to_owned());
        }
        let quality = match self.quality.as_deref() {
            Some(quality) => quality_named(quality).ok_or_else(|| {
                format!("--quality takes high, medium, small or 0 to 100, not {quality}")
            })?,
            None => Quality::Level(60),
        };
        let size = match self.size.as_deref() {
            Some(size) => Some(bytes_of(size).ok_or_else(|| {
                format!("--size takes a file size, such as 25MB, 800KB or 1.5GB, not {size}")
            })?),
            None => None,
        };
        let short_side = match self.short_side.as_deref() {
            Some(side) => Some(
                side.parse::<u32>()
                    .ok()
                    .filter(|side| *side >= 2)
                    .ok_or_else(|| format!("--short-side takes pixels, such as 720, not {side}"))?,
            ),
            None => None,
        };
        let fps = match self.fps.as_deref() {
            Some(rate) => Some(rate_of(rate).ok_or_else(|| {
                format!("--fps takes frames a second, such as 30 or 30000/1001, not {rate}")
            })?),
            None => None,
        };
        let threads = match self.threads.as_deref() {
            Some(threads) => Some(
                threads
                    .parse::<usize>()
                    .ok()
                    .filter(|threads| (1..=64).contains(threads))
                    .ok_or_else(|| format!("--threads takes 1 to 64, not {threads}"))?,
            ),
            None => None,
        };
        Ok(Compress {
            input,
            output: self.output,
            container,
            codec,
            quality,
            size,
            short_side,
            fps,
            threads,
            overwrite: self.overwrite,
        })
    }

    fn extract(self) -> Result<Extract, String> {
        let input = self
            .input
            .ok_or("extract-audio needs a file: dusq extract-audio <file>")?;
        let named = match self.output.as_deref().and_then(extension) {
            Some(extension) => Some(sound_named(&extension).ok_or_else(|| {
                format!(
                    "dusq cannot write .{extension} files; name the output .mp3, .m4a, .opus or .wav"
                )
            })?),
            None => None,
        };
        let format = match self.format.as_deref() {
            Some(format) => {
                let sound = sound_named(&format.to_lowercase())
                    .ok_or_else(|| format!("--format takes mp3, aac, opus or wav, not {format}"))?;
                if named.is_some_and(|named| named != sound) {
                    return Err(format!(
                        "the output is named for another format than --format {format}; make them agree"
                    ));
                }
                sound
            }
            None => named.unwrap_or(AudioFormat::Mp3),
        };
        Ok(Extract {
            input,
            output: self.output,
            format,
            overwrite: self.overwrite,
        })
    }
}

/// The extension of `path`, lowercase.
fn extension(path: &std::path::Path) -> Option<String> {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
}

fn container_named(name: &str) -> Option<Container> {
    match name {
        "mp4" | "m4v" => Some(Container::Mp4),
        "mov" => Some(Container::Mov),
        "mkv" => Some(Container::Mkv),
        "webm" => Some(Container::WebM),
        _ => None,
    }
}

fn codec_named(name: &str) -> Option<VideoCodec> {
    match name {
        "h264" | "h.264" | "avc" => Some(VideoCodec::H264),
        "hevc" | "h265" | "h.265" => Some(VideoCodec::Hevc),
        "av1" => Some(VideoCodec::Av1),
        "vp9" => Some(VideoCodec::Vp9),
        _ => None,
    }
}

fn sound_named(name: &str) -> Option<AudioFormat> {
    match name {
        "mp3" => Some(AudioFormat::Mp3),
        "aac" | "m4a" => Some(AudioFormat::M4a),
        "opus" | "ogg" | "oga" => Some(AudioFormat::Opus),
        "wav" => Some(AudioFormat::Wav),
        _ => None,
    }
}

/// The Quality presets by name, or the slider's level, 0 to 100.
fn quality_named(name: &str) -> Option<Quality> {
    match name.to_lowercase().as_str() {
        "high" => Some(Quality::Level(80)),
        "medium" => Some(Quality::Level(60)),
        "small" => Some(Quality::Level(40)),
        level => level
            .parse::<u8>()
            .ok()
            .filter(|level| *level <= 100)
            .map(Quality::Level),
    }
}

/// A file size written with KB, MB or GB (decimal, as sizes on disks are counted), or as a
/// bare number of megabytes.
fn bytes_of(size: &str) -> Option<u64> {
    let size = size.to_lowercase();
    let units = [
        ("kb", 1e3),
        ("k", 1e3),
        ("mb", 1e6),
        ("m", 1e6),
        ("gb", 1e9),
        ("g", 1e9),
    ];
    let (number, unit) = units
        .iter()
        .find_map(|(suffix, unit)| Some((size.strip_suffix(suffix)?, *unit)))
        .unwrap_or((size.as_str(), 1e6));
    let number: f64 = number.trim().parse().ok()?;
    let bytes = (number * unit).round();
    (number.is_finite() && bytes >= 1.0).then_some(bytes as u64)
}

/// A frame rate written as frames a second, whole or as a fraction.
fn rate_of(rate: &str) -> Option<(u32, u32)> {
    let (num, den) = match rate.split_once('/') {
        Some((num, den)) => (num.parse().ok()?, den.parse().ok()?),
        None => (rate.parse().ok()?, 1),
    };
    (num > 0 && den > 0).then_some((num, den))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_line(line: &str) -> Result<Command, String> {
        let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        parse(&args)
    }

    fn compress(line: &str) -> Compress {
        match parse_line(line) {
            Ok(Command::Compress(compress)) => compress,
            other => panic!("{line}: {other:?}"),
        }
    }

    fn extract(line: &str) -> Extract {
        match parse_line(line) {
            Ok(Command::ExtractAudio(extract)) => extract,
            other => panic!("{line}: {other:?}"),
        }
    }

    #[test]
    fn compress_has_defaults_for_everything_but_the_video() {
        let compress = compress("compress clip.mp4");
        assert_eq!(
            compress,
            Compress {
                input: "clip.mp4".into(),
                output: None,
                container: Container::Mp4,
                codec: VideoCodec::H264,
                quality: Quality::Level(60),
                size: None,
                short_side: None,
                fps: None,
                threads: None,
                overwrite: false,
            }
        );
    }

    #[test]
    fn every_compress_option_is_read() {
        let compress = compress(
            "compress in.mov -o out.mkv --quality 72 --short-side 720 --codec hevc --fps 30000/1001 --threads 2 --overwrite",
        );
        assert_eq!(compress.output, Some("out.mkv".into()));
        assert_eq!(compress.container, Container::Mkv);
        assert_eq!(compress.codec, VideoCodec::Hevc);
        assert_eq!(compress.quality, Quality::Level(72));
        assert_eq!(compress.short_side, Some(720));
        assert_eq!(compress.fps, Some((30_000, 1001)));
        assert_eq!(compress.threads, Some(2));
        assert!(compress.overwrite);
    }

    #[test]
    fn a_size_is_read_in_bytes_and_excludes_a_quality() {
        assert_eq!(compress("compress a.mp4").size, None);
        assert_eq!(
            compress("compress a.mp4 --size 25MB").size,
            Some(25_000_000)
        );
        assert_eq!(
            compress("compress a.mp4 --size=25mb").size,
            Some(25_000_000)
        );
        assert_eq!(
            compress("compress a.mp4 --size 1.5GB").size,
            Some(1_500_000_000)
        );
        assert_eq!(compress("compress a.mp4 --size 800KB").size, Some(800_000));
        assert_eq!(compress("compress a.mp4 --size 8M").size, Some(8_000_000));
        // A bare number is megabytes.
        assert_eq!(compress("compress a.mp4 --size 10").size, Some(10_000_000));
        for wrong in ["big", "0MB", "-5MB", "5TB"] {
            let error = parse_line(&format!("compress a.mp4 --size {wrong}")).unwrap_err();
            assert!(error.contains("--size"), "{wrong}: {error}");
        }
        let error = parse_line("compress a.mp4 --size 25MB --quality high").unwrap_err();
        assert!(
            error.contains("--quality") && error.contains("--size"),
            "{error}"
        );
    }

    #[test]
    fn quality_presets_have_names() {
        assert_eq!(
            compress("compress a.mp4 --quality high").quality,
            Quality::Level(80)
        );
        assert_eq!(
            compress("compress a.mp4 --quality Small").quality,
            Quality::Level(40)
        );
        assert!(parse_line("compress a.mp4 --quality 101").is_err());
        assert!(parse_line("compress a.mp4 --quality best").is_err());
    }

    #[test]
    fn the_format_follows_the_output_unless_given() {
        assert_eq!(
            compress("compress a.mp4 -o b.webm").container,
            Container::WebM
        );
        assert_eq!(compress("compress a.mp4 -o b.webm").codec, VideoCodec::Vp9);
        assert_eq!(
            compress("compress a.mp4 --format mov").container,
            Container::Mov
        );
        assert_eq!(
            compress("compress a.mp4 --output b.MP4").container,
            Container::Mp4
        );
        assert!(parse_line("compress a.mp4 -o b.mkv --format mp4").is_err());
        assert!(parse_line("compress a.mp4 --format webm --codec h264").is_err());
    }

    #[test]
    fn a_whole_frame_rate_needs_no_fraction() {
        assert_eq!(compress("compress a.mp4 --fps 25").fps, Some((25, 1)));
        assert!(parse_line("compress a.mp4 --fps 0").is_err());
        assert!(parse_line("compress a.mp4 --fps fast").is_err());
    }

    #[test]
    fn extract_audio_writes_mp3_unless_told() {
        let plain = extract("extract-audio clip.mp4");
        assert_eq!(plain.format, AudioFormat::Mp3);
        assert_eq!(plain.output, None);
        assert_eq!(
            extract("extract-audio clip.mp4 --format opus").format,
            AudioFormat::Opus
        );
        assert_eq!(
            extract("extract-audio clip.mp4 -o voice.m4a").format,
            AudioFormat::M4a
        );
        assert_eq!(
            extract("extract-audio clip.mp4 -o voice.wav").format,
            AudioFormat::Wav
        );
        assert!(parse_line("extract-audio clip.mp4 -o voice.ogg --format mp3").is_err());
    }

    #[test]
    fn help_and_version_need_nothing_else() {
        assert_eq!(parse_line(""), Ok(Command::Help));
        assert_eq!(parse_line("--help"), Ok(Command::Help));
        assert_eq!(parse_line("compress -h"), Ok(Command::Help));
        assert_eq!(parse_line("-V"), Ok(Command::Version));
    }

    #[test]
    fn mistakes_say_what_is_wrong() {
        let error = parse_line("squash a.mp4").unwrap_err();
        assert!(error.contains("squash"), "{error}");
        let error = parse_line("compress").unwrap_err();
        assert!(error.contains("video"), "{error}");
        let error = parse_line("compress a.mp4 --short-side").unwrap_err();
        assert!(error.contains("--short-side"), "{error}");
        let error = parse_line("compress a.mp4 --loud").unwrap_err();
        assert!(error.contains("--loud"), "{error}");
        let error = parse_line("compress a.mp4 b.mp4").unwrap_err();
        assert!(error.contains("b.mp4"), "{error}");
        let error = parse_line("extract-audio a.mp4 --threads 2").unwrap_err();
        assert!(error.contains("--threads"), "{error}");
    }

    #[test]
    fn an_option_without_a_value_takes_none() {
        // "--overwrite=no" must not overwrite.
        let error = parse_line("compress a.mp4 --overwrite=no").unwrap_err();
        assert!(error.contains("--overwrite"), "{error}");
        assert!(parse_line("extract-audio a.mp4 --overwrite=yes").is_err());
    }

    #[test]
    fn a_file_whose_name_starts_with_a_dash_follows_two_dashes() {
        assert_eq!(
            compress("compress -- -intro.mp4").input,
            PathBuf::from("-intro.mp4")
        );
        assert_eq!(
            compress("compress --quality small -- -intro.mp4").quality,
            Quality::Level(40)
        );
        // After them, nothing is an option.
        assert!(parse_line("compress -- a.mp4 --overwrite").is_err());
    }
}
