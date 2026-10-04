//! FFmpeg wrapper: probe, decode, encode and mux.
//!
//! A leaf crate: it knows nothing about the UI, the project model or the other leaf crates.

#![deny(unsafe_code)]

mod audio;
mod decode;
mod error;
#[allow(unsafe_code)]
mod ffi;
mod input;
mod probe;

pub use audio::AudioDecoder;
pub use decode::{Acceleration, DecodedFrame, VideoDecoder};
pub use error::MediaError;
pub use probe::{ProbeInfo, StreamDetail, StreamKind, StreamSummary, probe};
