//! FFmpeg wrapper: probe, decode, encode and mux.
//!
//! A leaf crate: it knows nothing about the UI, the project model or the other leaf crates.

#![deny(unsafe_code)]

mod error;
#[allow(unsafe_code)]
mod ffi;
mod probe;

pub use error::MediaError;
pub use probe::{ProbeInfo, StreamDetail, StreamKind, StreamSummary, probe};
