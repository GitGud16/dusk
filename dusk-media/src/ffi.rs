//! The only place in Dusk where `unsafe` FFmpeg access is allowed (CLAUDE.md). Each function
//! wraps one raw read or call that `ffmpeg-next` does not expose and states the invariant it
//! relies on.

use ffmpeg_next::codec::Parameters;

/// The size and audio format fields of a stream's codec parameters, which `ffmpeg-next`
/// does not expose without opening a decoder.
pub(crate) struct CodecFields {
    pub width: i32,
    pub height: i32,
    pub sample_rate: i32,
    pub channels: i32,
}

/// Reads [`CodecFields`] from `parameters` without opening a decoder.
pub(crate) fn codec_fields(parameters: &Parameters) -> CodecFields {
    // SAFETY: `as_ptr` returns the AVCodecParameters that `parameters` points to. It is
    // non-null and initialized for as long as `parameters` (and the stream it borrows from)
    // is alive, which the borrow guarantees; only plain integer fields are read, and nothing
    // is written.
    unsafe {
        let raw = &*parameters.as_ptr();
        CodecFields {
            width: raw.width,
            height: raw.height,
            sample_rate: raw.sample_rate,
            channels: raw.ch_layout.nb_channels,
        }
    }
}
