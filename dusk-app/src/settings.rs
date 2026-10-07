//! What Dusk keeps in its settings folder (docs/ARCHITECTURE.md, "Keyboard and settings"):
//! the user's shortcuts and settings, each read once when Dusk starts and written on the file
//! worker.

use std::io;
use std::path::{Path, PathBuf};

use dusk_engine::{Container, ExportFormat, ExportSettings, Quality, VideoCodec};

use crate::export_choices::PRESETS;
use crate::files::write_atomically;
use crate::keymap::Keymap;

/// The shortcuts file's name in the settings folder.
pub const SHORTCUTS_FILE: &str = "shortcuts.txt";

/// The settings file's name in the settings folder.
pub const SETTINGS_FILE: &str = "settings.txt";

/// The frame cache's sizes a user can pick, in megabytes (2^20 bytes, as the cache counts).
pub const CACHE_MB: std::ops::RangeInclusive<u32> = 128..=4096;

/// What the export dialog starts from in a session, until an export there changes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportDefault {
    pub container: Container,
    pub codec: VideoCodec,
    /// The picture's short side, such as 1080; `None` for the sequence's own size.
    pub short_side: Option<u32>,
    /// The Quality slider, 0 to 100.
    pub level: u8,
}

/// The user's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The frame cache's size, in megabytes.
    pub cache_mb: u32,
    pub export: ExportDefault,
    /// The user's own `ffmpeg` program, for x264 and x265.
    pub ffmpeg: Option<PathBuf>,
    /// Whether exports use it for the codecs it has an encoder for.
    pub use_ffmpeg: bool,
}

impl Default for Settings {
    /// 384 MB of frames, and MP4 with H.264 at the sequence's size and High quality.
    fn default() -> Settings {
        Settings {
            cache_mb: 384,
            export: ExportDefault {
                container: Container::Mp4,
                codec: VideoCodec::H264,
                short_side: None,
                level: 80,
            },
            ffmpeg: None,
            use_ffmpeg: true,
        }
    }
}

/// The settings file, with `{…}` where each setting's value goes.
const TEMPLATE: &str = "\
# Dusk's settings: a name, \"=\" and its value on each line. Dusk reads this file when it
# starts, and writes it when you change Settings (File, Settings).

# How much memory decoded frames may take, in megabytes: 128 to 4096. More keeps more of the
# timeline ready to show; less keeps Dusk lighter.
cache = {cache}

# Where the export dialog starts: the file format (MP4, MOV, MKV or WebM), the codec (H.264,
# HEVC, AV1 or VP9), the picture's short side (sequence for the sequence's own size, or 1080,
# 720 or 480) and the quality (0 to 100; High is 80, Medium 60 and Small 40).
export-format = {format}
export-codec = {codec}
export-size = {size}
export-quality = {quality}

# Your own ffmpeg.exe, for x264 and x265 (nothing after the \"=\" for none), and whether
# exports use it for them (yes or no).
ffmpeg ={ffmpeg}
use-ffmpeg = {use}
";

/// The codec `text` names, in any case, with or without the dot in H.264.
fn codec_named(text: &str) -> Option<VideoCodec> {
    let plain = |name: &str| name.replace('.', "").to_ascii_lowercase();
    Container::Mkv
        .video_codecs()
        .iter()
        .copied()
        .find(|codec| plain(codec.name()) == plain(text))
}

impl Settings {
    /// The defaults with `text`, a settings file, over them, and what in it could not be
    /// read, a line each: a line Dusk cannot read keeps that setting's default.
    pub fn read(text: &str) -> (Settings, Vec<String>) {
        let mut settings = Settings::default();
        let mut problems: Vec<(usize, String)> = Vec::new();
        // Whether the format holds the codec is known once both are read.
        let mut codec: Option<(usize, VideoCodec)> = None;
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            let line = line.trim_start_matches('\u{feff}').trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                problems.push((
                    number,
                    "a setting's name, \"=\" and its value were expected; that line is skipped"
                        .into(),
                ));
                continue;
            };
            let (name, value) = (name.trim(), value.trim());
            let problem = match name {
                "cache" => match value.parse::<u32>() {
                    Ok(megabytes) if CACHE_MB.contains(&megabytes) => {
                        settings.cache_mb = megabytes;
                        None
                    }
                    _ => Some(format!(
                        "the frame cache takes {} to {} MB, not \"{value}\"",
                        CACHE_MB.start(),
                        CACHE_MB.end()
                    )),
                },
                "export-format" => match Container::ALL
                    .into_iter()
                    .find(|container| container.name().eq_ignore_ascii_case(value))
                {
                    Some(container) => {
                        settings.export.container = container;
                        None
                    }
                    None => Some(format!(
                        "\"{value}\" is not a file format Dusk exports: MP4, MOV, MKV or WebM"
                    )),
                },
                "export-codec" => match codec_named(value) {
                    Some(named) => {
                        codec = Some((number, named));
                        None
                    }
                    None => Some(format!(
                        "\"{value}\" is not a codec Dusk exports: H.264, HEVC, AV1 or VP9"
                    )),
                },
                "export-size" => match value.to_ascii_lowercase().as_str() {
                    "sequence" => {
                        settings.export.short_side = None;
                        None
                    }
                    other => match other.parse::<u32>() {
                        Ok(side) if PRESETS.contains(&side) => {
                            settings.export.short_side = Some(side);
                            None
                        }
                        _ => Some(format!(
                            "the picture size is sequence, 1080, 720 or 480, not \"{value}\""
                        )),
                    },
                },
                "export-quality" => match value.parse::<u8>() {
                    Ok(level) if level <= 100 => {
                        settings.export.level = level;
                        None
                    }
                    _ => Some(format!("the quality is 0 to 100, not \"{value}\"")),
                },
                "ffmpeg" => {
                    settings.ffmpeg = (!value.is_empty()).then(|| PathBuf::from(value));
                    None
                }
                "use-ffmpeg" => match value.to_ascii_lowercase().as_str() {
                    "yes" | "true" => {
                        settings.use_ffmpeg = true;
                        None
                    }
                    "no" | "false" => {
                        settings.use_ffmpeg = false;
                        None
                    }
                    _ => Some(format!("use-ffmpeg is yes or no, not \"{value}\"")),
                },
                _ => {
                    let unknown =
                        format!("Dusk has no setting called \"{name}\"; that line is skipped");
                    problems.push((number, unknown));
                    continue;
                }
            };
            if let Some(problem) = problem {
                problems.push((number, format!("{problem}; the default is used")));
            }
        }
        let container = settings.export.container;
        if let Some((number, codec)) = codec {
            if container.video_codecs().contains(&codec) {
                settings.export.codec = codec;
            } else {
                problems.push((
                    number,
                    format!(
                        "{} does not hold {}; the default is used",
                        container.name(),
                        codec.name()
                    ),
                ));
            }
        }
        // A format that cannot hold the codec it is left with takes its first, as choosing the
        // format in the Settings dialog does.
        if let Some(first) = container
            .video_codecs()
            .first()
            .filter(|_| !container.video_codecs().contains(&settings.export.codec))
        {
            settings.export.codec = *first;
        }
        problems.sort_by_key(|(number, _)| *number);
        let problems = problems
            .into_iter()
            .map(|(number, problem)| format!("line {number}: {problem}"))
            .collect();
        (settings, problems)
    }

    /// The settings file that keeps these settings, each with a comment saying what it takes.
    pub fn to_file(&self) -> String {
        let size = self
            .export
            .short_side
            .map_or_else(|| "sequence".to_owned(), |side| side.to_string());
        let ffmpeg = self
            .ffmpeg
            .as_ref()
            .map_or_else(String::new, |path| format!(" {}", path.display()));
        TEMPLATE
            .replace("{cache}", &self.cache_mb.to_string())
            .replace("{format}", self.export.container.name())
            .replace("{codec}", self.export.codec.name())
            .replace("{size}", &size)
            .replace("{quality}", &self.export.level.to_string())
            .replace("{ffmpeg}", &ffmpeg)
            .replace("{use}", if self.use_ffmpeg { "yes" } else { "no" })
    }

    /// The frame cache's cap in bytes.
    pub fn cache_bytes(&self) -> usize {
        usize::try_from(self.cache_mb).unwrap_or(384) * 1024 * 1024
    }

    /// The export settings the export dialog starts from.
    pub fn export_settings(&self) -> ExportSettings {
        let container = self.export.container;
        let audio = container
            .audio_codecs()
            .first()
            .copied()
            .unwrap_or(dusk_engine::AudioCodec::Aac);
        ExportSettings {
            format: ExportFormat::Video {
                container,
                codec: self.export.codec,
                audio,
            },
            short_side: self.export.short_side,
            quality: Quality::Level(self.export.level),
            audio_bit_rate: None,
        }
    }
}

/// The keymap the shortcuts file in `dir` keeps, and what in it could not be read, each a
/// sentence for the status line; the defaults when there is no folder or no file.
pub fn read_keymap(dir: Option<&Path>) -> (Keymap, Vec<String>) {
    let Some(dir) = dir else {
        return (Keymap::default(), Vec::new());
    };
    let path = dir.join(SHORTCUTS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let (map, problems) = Keymap::read(&text);
            let problems = problems
                .into_iter()
                .map(|problem| format!("{SHORTCUTS_FILE}, {problem}; that line is skipped."))
                .collect();
            (map, problems)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (Keymap::default(), Vec::new())
        }
        Err(error) => (
            Keymap::default(),
            vec![format!(
                "Dusk could not read {}: {error}; the default keys are used.",
                path.display()
            )],
        ),
    }
}

/// Writes the shortcuts file that keeps `keymap` into `dir`, which it makes if needed: as
/// "<name>.part" first, renamed into place once flushed. It does file work, so it runs on the
/// file worker.
pub fn write_keymap(dir: &Path, keymap: &Keymap) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    write_atomically(&dir.join(SHORTCUTS_FILE), keymap.to_file().as_bytes())
}

/// The settings the settings file in `dir` keeps, and what in it could not be read, each a
/// sentence for the status line; the defaults when there is no folder or no file.
pub fn read_settings(dir: Option<&Path>) -> (Settings, Vec<String>) {
    let Some(dir) = dir else {
        return (Settings::default(), Vec::new());
    };
    let path = dir.join(SETTINGS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let (settings, problems) = Settings::read(&text);
            let problems = problems
                .into_iter()
                .map(|problem| format!("{SETTINGS_FILE}, {problem}."))
                .collect();
            (settings, problems)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (Settings::default(), Vec::new())
        }
        Err(error) => (
            Settings::default(),
            vec![format!(
                "Dusk could not read {}: {error}; the default settings are used.",
                path.display()
            )],
        ),
    }
}

/// Writes the settings file that keeps `settings` into `dir`, which it makes if needed (see
/// [`write_keymap`]); it runs on the file worker.
pub fn write_settings(dir: &Path, settings: &Settings) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    write_atomically(&dir.join(SETTINGS_FILE), settings.to_file().as_bytes())
}

/// What the status line says of `problems`: the first, and how many more there are.
pub fn problems_message(problems: &[String]) -> Option<String> {
    let (first, rest) = problems.split_first()?;
    Some(match rest.len() {
        0 => first.clone(),
        1 => format!("{first} 1 more line could not be read."),
        more => format!("{first} {more} more lines could not be read."),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcuts::Action;

    /// A folder of its own for one test.
    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dusk-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn without_a_file_the_keys_are_the_defaults() {
        let dir = folder("none");
        assert_eq!(read_keymap(Some(&dir)), (Keymap::default(), Vec::new()));
        assert_eq!(read_keymap(None), (Keymap::default(), Vec::new()));
    }

    #[test]
    fn the_file_changes_the_keys_and_says_what_it_skipped() {
        let dir = folder("some");
        std::fs::write(dir.join(SHORTCUTS_FILE), "split = X\nsplitt = Y\n").unwrap();
        let (map, problems) = read_keymap(Some(&dir));
        assert_eq!(map.keys_of(Action::Split).len(), 1);
        assert_eq!(map.keys_of(Action::Split)[0].to_string(), "X");
        assert_eq!(
            problems,
            ["shortcuts.txt, line 2: Dusk has no action called \"splitt\"; that line is skipped."]
        );
    }

    #[test]
    fn a_file_that_cannot_be_read_leaves_the_defaults() {
        let dir = folder("broken");
        std::fs::write(dir.join(SHORTCUTS_FILE), [0xFF, 0xFE, 0x00, 0x41]).unwrap();
        let (map, problems) = read_keymap(Some(&dir));
        assert_eq!(map, Keymap::default());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("shortcuts.txt"), "{}", problems[0]);
        assert!(
            problems[0].ends_with("the default keys are used."),
            "{}",
            problems[0]
        );
    }

    #[test]
    fn a_written_keymap_reads_back_the_same() {
        let dir = folder("written").join("Dusk");
        let (changed, _) = Keymap::read(
            "split = X
unlink =
",
        );
        write_keymap(&dir, &changed).unwrap();
        assert_eq!(read_keymap(Some(&dir)), (changed, Vec::new()));
        // The defaults make a file of comments that names every action.
        write_keymap(&dir, &Keymap::default()).unwrap();
        let text = std::fs::read_to_string(dir.join(SHORTCUTS_FILE)).unwrap();
        assert!(text.contains("# split = S"), "{text}");
        assert_eq!(read_keymap(Some(&dir)), (Keymap::default(), Vec::new()));
    }

    #[test]
    fn without_a_settings_file_the_settings_are_the_defaults() {
        let (settings, problems) = Settings::read("");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.cache_mb, 384);
        assert_eq!(settings.cache_bytes(), 384 * 1024 * 1024);
        assert_eq!(settings.export.container, Container::Mp4);
        assert_eq!(settings.export.codec, VideoCodec::H264);
        assert_eq!(
            (settings.export.short_side, settings.export.level),
            (None, 80)
        );
        assert_eq!(
            (settings.ffmpeg.as_deref(), settings.use_ffmpeg),
            (None, true)
        );
    }

    #[test]
    fn a_settings_file_gives_every_setting() {
        let text = "cache = 1024\n\
                    export-format = mkv\n\
                    export-codec = vp9\n\
                    export-size = 720\n\
                    export-quality = 40\n\
                    ffmpeg = C:\\tools\\ffmpeg.exe\n\
                    use-ffmpeg = no\n";
        let (settings, problems) = Settings::read(text);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(settings.cache_mb, 1024);
        assert_eq!(
            settings.export,
            ExportDefault {
                container: Container::Mkv,
                codec: VideoCodec::Vp9,
                short_side: Some(720),
                level: 40,
            }
        );
        assert_eq!(
            settings.ffmpeg.as_deref(),
            Some(Path::new("C:\\tools\\ffmpeg.exe"))
        );
        assert!(!settings.use_ffmpeg);
    }

    #[test]
    fn a_format_alone_takes_its_first_codec_when_it_cannot_hold_the_default() {
        // As choosing the format in the Settings dialog does.
        let (settings, problems) = Settings::read("export-format = WebM\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(settings.export.container, Container::WebM);
        assert_eq!(settings.export.codec, VideoCodec::Vp9);
        // A format that holds the default keeps it.
        let (settings, _) = Settings::read("export-format = MKV\n");
        assert_eq!(settings.export.codec, VideoCodec::H264);
    }

    #[test]
    fn settings_that_cannot_be_read_keep_their_defaults() {
        let text = "cache = 50\n\
                    export-codec = vp9\n\
                    export-size = 900\n\
                    export-quality = loud\n\
                    colour = blue\n\
                    use-ffmpeg = maybe\n\
                    cache\n";
        let (settings, problems) = Settings::read(text);
        assert_eq!(problems.len(), 7, "{problems:?}");
        for (problem, line) in problems.iter().zip(1..) {
            assert!(problem.starts_with(&format!("line {line}: ")), "{problem}");
        }
        // Each says what Dusk does instead: a value it cannot read keeps the default, and a
        // line that names no setting, or nothing at all, is skipped.
        for (index, problem) in problems.iter().enumerate() {
            let instead = if [4, 6].contains(&index) {
                "; that line is skipped"
            } else {
                "; the default is used"
            };
            assert!(problem.ends_with(instead), "{problem}");
        }
        // MP4 does not hold VP9, so the default codec stays.
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn written_settings_read_back_the_same() {
        let settings = Settings {
            cache_mb: 512,
            export: ExportDefault {
                container: Container::WebM,
                codec: VideoCodec::Av1,
                short_side: Some(1080),
                level: 60,
            },
            ffmpeg: Some(PathBuf::from("D:\\my tools\\ffmpeg.exe")),
            use_ffmpeg: true,
        };
        assert_eq!(Settings::read(&settings.to_file()), (settings, Vec::new()));
        let defaults = Settings::default().to_file();
        assert!(defaults.contains("\ncache = 384\n"), "{defaults}");
        assert!(defaults.contains("\nffmpeg =\n"), "{defaults}");
        assert_eq!(Settings::read(&defaults), (Settings::default(), Vec::new()));
    }

    #[test]
    fn settings_are_read_from_and_written_to_the_folder() {
        let dir = folder("settings").join("Dusk");
        assert_eq!(read_settings(Some(&dir)), (Settings::default(), Vec::new()));
        assert_eq!(read_settings(None), (Settings::default(), Vec::new()));
        let settings = Settings {
            cache_mb: 256,
            ..Settings::default()
        };
        write_settings(&dir, &settings).unwrap();
        assert_eq!(read_settings(Some(&dir)), (settings, Vec::new()));
        std::fs::write(
            dir.join(SETTINGS_FILE),
            "cache = lots
",
        )
        .unwrap();
        let (read, problems) = read_settings(Some(&dir));
        assert_eq!(read, Settings::default());
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].starts_with("settings.txt, line 1: ") && problems[0].ends_with("is used."),
            "{}",
            problems[0]
        );
    }

    #[test]
    fn the_export_default_becomes_export_settings() {
        let mut settings = Settings::default();
        let export = settings.export_settings();
        assert_eq!(export.quality, Quality::Level(80));
        assert_eq!(export.short_side, None);
        settings.export = ExportDefault {
            container: Container::WebM,
            codec: VideoCodec::Vp9,
            short_side: Some(480),
            level: 40,
        };
        let export = settings.export_settings();
        assert_eq!(
            export.format,
            ExportFormat::Video {
                container: Container::WebM,
                codec: VideoCodec::Vp9,
                audio: dusk_engine::AudioCodec::Opus,
            }
        );
        assert_eq!(
            (export.short_side, export.quality),
            (Some(480), Quality::Level(40))
        );
    }

    #[test]
    fn the_status_line_names_the_first_problem_and_counts_the_rest() {
        assert_eq!(problems_message(&[]), None);
        let one = vec!["shortcuts.txt, line 2: X; that line is skipped.".to_owned()];
        assert_eq!(problems_message(&one).as_deref(), Some(one[0].as_str()));
        let three = vec![one[0].clone(), "b".to_owned(), "c".to_owned()];
        assert_eq!(
            problems_message(&three).as_deref(),
            Some("shortcuts.txt, line 2: X; that line is skipped. 2 more lines could not be read.")
        );
    }
}
