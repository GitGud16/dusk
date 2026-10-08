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
    /// The file has no video stream to decode.
    #[error("{} has no video; pick a video file", path.display())]
    NoVideo {
        /// The path as given.
        path: PathBuf,
    },
    /// The file has no audio stream to decode.
    #[error("{} has no audio; pick a file with sound", path.display())]
    NoAudio {
        /// The path as given.
        path: PathBuf,
    },
    /// FFmpeg could not open or read the file.
    #[error("FFmpeg could not read {}: {source}. The file may be damaged or in a format FFmpeg cannot read; try another copy of it, or convert it to MP4", path.display())]
    Open {
        /// The path as given.
        path: PathBuf,
        /// FFmpeg's error.
        #[source]
        source: ffmpeg_next::Error,
    },
    /// FFmpeg failed while decoding the file.
    #[error("FFmpeg could not decode {}: {source}. The file may be damaged; try another copy of it", path.display())]
    Decode {
        /// The path as given.
        path: PathBuf,
        /// FFmpeg's error.
        #[source]
        source: ffmpeg_next::Error,
    },
    /// The output file could not be created.
    #[error("{} could not be created: {source}. Check that the folder exists and that you can write to it", path.display())]
    Create {
        /// The path as given.
        path: PathBuf,
        /// FFmpeg's error.
        #[source]
        source: ffmpeg_next::Error,
    },
    /// No encoder of the codec could be opened.
    #[error(
        "no {codec} encoder could be opened ({tried}); update the graphics driver, or choose another codec"
    )]
    NoEncoder {
        /// The codec, such as H.264.
        codec: &'static str,
        /// Each encoder tried, with why it failed.
        tried: String,
    },
    /// The file format cannot hold the codec asked for.
    #[error("{format} files cannot hold {codec}; choose another codec or file format")]
    Unsupported {
        /// The file format, such as WebM.
        format: &'static str,
        /// The codec, such as H.264.
        codec: &'static str,
    },
    /// FFmpeg failed while encoding or writing the file.
    #[error("FFmpeg could not write {}: {source}. Check that the disk has room and try again", path.display())]
    Encode {
        /// The path as given.
        path: PathBuf,
        /// FFmpeg's error.
        #[source]
        source: ffmpeg_next::Error,
    },
    /// The video uses a pixel format Dusk cannot show yet.
    #[error("{} stores its pictures as {format}, which Dusk cannot show yet; convert it to a common format such as H.264 and try again", path.display())]
    UnsupportedPixelFormat {
        /// The path as given.
        path: PathBuf,
        /// FFmpeg's name for the pixel format.
        format: String,
    },
}
