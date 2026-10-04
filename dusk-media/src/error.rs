use std::path::PathBuf;

/// Why a media operation failed. Messages say what happened and what the user can do.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    /// The path does not name an existing local file (this includes URLs, which Dusk never opens).
    #[error("{} is not a file that exists; check the path and try again", path.display())]
    NotAFile {
        /// The path as given.
        path: PathBuf,
    },
    /// The path cannot be passed to FFmpeg because it is not valid Unicode.
    #[error("{} has a name that is not valid Unicode; rename the file and try again", path.display())]
    NonUnicodePath {
        /// The path as given.
        path: PathBuf,
    },
    /// FFmpeg could not open or read the file.
    #[error("FFmpeg could not read {}: {source}. The file may be damaged or in a format FFmpeg cannot read", path.display())]
    Open {
        /// The path as given.
        path: PathBuf,
        /// FFmpeg's error.
        #[source]
        source: ffmpeg_next::Error,
    },
}
