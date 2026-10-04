//! Worker-thread orchestration: frame cache, decoder pool, playback scheduler, thumbnail
//! and waveform jobs, and the export pipeline. The only crate that moves data between the
//! leaf crates (`dusk-media`, `dusk-render`, `dusk-audio`).

mod error;
mod info;
#[cfg(feature = "gpu")]
mod preview;

#[cfg(feature = "gpu")]
pub use dusk_render::{Gpu, GpuError};
pub use error::EngineError;
pub use info::media_info;
#[cfg(feature = "gpu")]
pub use preview::{DECODER_IDLE, Preview, PreviewError, PreviewEvent};
