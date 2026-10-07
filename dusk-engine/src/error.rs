use std::path::{Path, PathBuf};

use dusk_media::MediaError;

/// Why the engine could not do what it was asked. Messages say what happened and what the
/// user can do.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// Reading, decoding or writing media failed.
    #[error(transparent)]
    Media(#[from] MediaError),
    /// The file is not something Dusk can import yet.
    #[error("{} cannot be imported: {reason}", path.display())]
    Unsupported {
        /// The file.
        path: PathBuf,
        /// What is missing or not supported yet.
        reason: &'static str,
    },
    /// Drawing a frame failed.
    #[cfg(feature = "gpu")]
    #[error(transparent)]
    Render(#[from] dusk_render::RenderError),
    /// The audio device failed.
    #[cfg(feature = "gpu")]
    #[error(transparent)]
    Audio(#[from] dusk_audio::AudioError),
    /// An export is already running.
    #[error("an export is already running; wait for it to finish or cancel it")]
    ExportRunning,
    /// There is nothing to export.
    #[error("the timeline is empty; add a clip before exporting")]
    Empty,
    /// A sound export of a timeline without sound.
    #[error("the timeline has no sound to export; place an audio clip or unmute an audio track")]
    NoSound,
    /// A file could not be renamed or removed.
    #[error("Dusk could not finish {}: {source}. Check that you can write to that folder", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// What the system said.
        #[source]
        source: std::io::Error,
    },
    /// A target size below the smallest file the video can become (docs/ARCHITECTURE.md,
    /// "Target file size": 240p at 200 kbit/s, with 48 kbit/s of sound).
    #[error(
        "the smallest this video can become is {} MB; aim at that size or more",
        megabytes_up(*smallest)
    )]
    TooSmall {
        /// The smallest size, in bytes.
        smallest: u64,
    },
    /// A source that does not say how long it is, which a target size needs.
    #[error("{} does not say how long it is, so it cannot be made to a size; compress it at a quality instead", path.display())]
    NoLength {
        /// The file.
        path: PathBuf,
    },
    /// A worker thread could not start.
    #[error("Dusk could not start a worker thread ({0}); close some programs and try again")]
    Thread(std::io::Error),
    /// The user's own `ffmpeg` program could not be run.
    #[error("Dusk could not run {}: {source}. Pick your ffmpeg.exe again", path.display())]
    ExternalProgram {
        /// The program.
        path: PathBuf,
        /// What the system said.
        #[source]
        source: std::io::Error,
    },
    /// The program picked does not list its encoders as `ffmpeg` does.
    #[error("{} is not an ffmpeg program that Dusk can use; pick an ffmpeg.exe", path.display())]
    NotFfmpeg {
        /// The program.
        path: PathBuf,
    },
    /// The user's own `ffmpeg` stopped with an error.
    #[error(
        "your ffmpeg stopped: {message}. Export with Dusk's own encoder, or check that program"
    )]
    ExternalFailed {
        /// What it said last, or how it ended.
        message: String,
    },
}

/// `bytes` in megabytes with one decimal, rounded up, so the size said is never below it.
fn megabytes_up(bytes: u64) -> String {
    let tenths = bytes.div_ceil(100_000);
    format!("{}.{}", tenths / 10, tenths % 10)
}

impl EngineError {
    /// The file that is not where it was said to be, when that is what went wrong: a media
    /// file of the project that went away, for one.
    pub fn missing_file(&self) -> Option<&Path> {
        match self {
            EngineError::Media(MediaError::NotAFile { path }) => Some(path),
            _ => None,
        }
    }

    /// This error, or the file going away when the file it failed to read is no longer there
    /// (`exists` says): a drive unplugged under an open decoder fails as FFmpeg failing to
    /// read the file, though what the user can do is find it.
    pub fn or_gone(self, exists: impl Fn(&Path) -> bool) -> EngineError {
        match self {
            EngineError::Media(MediaError::Open { path, .. } | MediaError::Decode { path, .. })
                if !exists(&path) =>
            {
                EngineError::Media(MediaError::NotAFile { path })
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_is_not_there_is_named() {
        let missing = EngineError::Media(MediaError::NotAFile {
            path: PathBuf::from("E:/trip/beach.mp4"),
        });
        assert_eq!(missing.missing_file(), Some(Path::new("E:/trip/beach.mp4")));
        let other = EngineError::Media(MediaError::NoAudio {
            path: PathBuf::from("E:/trip/beach.mp4"),
        });
        assert_eq!(other.missing_file(), None);
        assert_eq!(EngineError::Empty.missing_file(), None);
    }

    #[test]
    fn a_file_that_went_away_while_it_was_read_is_named_as_missing() {
        // A drive unplugged under an open decoder fails as FFmpeg failing to read the file.
        let dir = std::env::temp_dir().join("dusk-engine-gone");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("damaged.mp4");
        std::fs::write(&path, b"not a video").unwrap();
        let read = || {
            let failed = dusk_media::VideoDecoder::open(&path, dusk_media::Acceleration::Software);
            EngineError::from(failed.err().expect("not a video"))
        };
        // Still there, the file is damaged, not missing.
        assert_eq!(read().or_gone(Path::is_file).missing_file(), None);
        let error = read();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            error.or_gone(Path::is_file).missing_file(),
            Some(path.as_path())
        );
        assert_eq!(EngineError::Empty.or_gone(|_| false).missing_file(), None);
    }

    #[test]
    fn the_smallest_size_is_never_said_below_itself() {
        let refused = EngineError::TooSmall {
            smallest: 1_917_526,
        };
        assert_eq!(
            refused.to_string(),
            "the smallest this video can become is 2.0 MB; aim at that size or more"
        );
        assert_eq!(megabytes_up(25_000_000), "25.0");
    }
}
