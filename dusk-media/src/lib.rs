//! FFmpeg wrapper: probe, decode, encode and mux.
//!
//! A leaf crate: it knows nothing about the UI, the project model or the other leaf crates.

#![deny(unsafe_code)]

mod audio;
mod decode;
mod encode;
mod error;
#[allow(unsafe_code)]
mod ffi;
mod formats;
mod input;
mod orientation;
mod probe;
mod still;

pub use audio::AudioDecoder;
pub use decode::{Acceleration, DecodedFrame, Following, HUGE_FRAME, Step, VideoDecoder};
pub use encode::{AudioSettings, VideoSettings, Writer, available_encoders};
pub use error::MediaError;
pub use formats::{
    AudioCodec, AudioFormat, Container, ENCODERS, Encoder, Quality, VideoCodec, encoder_named,
    encoders_of,
};
pub use probe::{ProbeInfo, StreamDetail, StreamKind, StreamSummary, probe};
pub use still::{StillInfo, decode_still, is_still, still_info};
