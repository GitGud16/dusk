use std::path::PathBuf;

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
}

/// `bytes` in megabytes with one decimal, rounded up, so the size said is never below it.
fn megabytes_up(bytes: u64) -> String {
    let tenths = bytes.div_ceil(100_000);
    format!("{}.{}", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

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
