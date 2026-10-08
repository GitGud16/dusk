//! Opening local media files for reading, the same way for probing and decoding.

use std::path::Path;
use std::sync::Once;

use ffmpeg_next as ffmpeg;

use crate::MediaError;

/// Opens the local file at `path` for reading.
///
/// Only local files are opened: FFmpeg's protocol whitelist is set to `file`, so no network
/// protocol is ever used, even for a URL passed in by mistake.
pub(crate) fn open_input(path: &Path) -> Result<ffmpeg::format::context::Input, MediaError> {
    // Only existing local files reach FFmpeg, which also keeps URLs away from it.
    if !path.is_file() {
        return Err(MediaError::NotAFile {
            path: path.to_path_buf(),
        });
    }
    // ffmpeg-next panics on paths that are not valid Unicode, so check first.
    let path_str = path.to_str().ok_or_else(|| MediaError::NonUnicodePath {
        path: path.to_path_buf(),
    })?;
    init();

    // Defense in depth: whatever the string looks like, FFmpeg may only use the file protocol.
    let mut options = ffmpeg::Dictionary::new();
    options.set("protocol_whitelist", "file");
    ffmpeg::format::input_with_dictionary(path_str, options).map_err(|source| MediaError::Open {
        path: path.to_path_buf(),
        source,
    })
}

/// One-time FFmpeg setup for this process.
pub(crate) fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // With FFmpeg 5 and later this only registers error strings; it always returns Ok.
        let _ = ffmpeg::init();
        // FFmpeg logs to stderr by default; keep errors only until Dusk routes its logs.
        ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Error);
    });
}
