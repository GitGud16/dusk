//! Worker-thread orchestration: frame cache, decoder pool, playback scheduler, thumbnail
//! and waveform jobs, and the export pipeline. The only crate that moves data between the
//! leaf crates (`dusk-media`, `dusk-render`, `dusk-audio`).

#[cfg(feature = "gpu")]
mod cache;
#[cfg(feature = "gpu")]
mod engine;
mod error;
#[cfg(feature = "gpu")]
mod export;
mod info;
#[cfg(feature = "gpu")]
mod mixer;
#[cfg(feature = "gpu")]
mod placement;
#[cfg(feature = "gpu")]
mod sound;
#[cfg(feature = "gpu")]
mod video;

#[cfg(feature = "gpu")]
pub use dusk_render::{Gpu, GpuError, wgpu};
#[cfg(feature = "gpu")]
pub use engine::{DECODER_IDLE, Engine, EngineEvent, EngineOptions};
pub use error::EngineError;
#[cfg(feature = "gpu")]
pub use export::{ExportEvent, ExportJob};
pub use info::media_info;
