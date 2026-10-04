//! Worker-thread orchestration: frame cache, decoder pool, playback scheduler, thumbnail
//! and waveform jobs, and the export pipeline. The only crate that moves data between the
//! leaf crates (`dusk-media`, `dusk-render`, `dusk-audio`).

#[cfg(feature = "gpu")]
pub use dusk_render::{Gpu, GpuError};
