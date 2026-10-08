//! The wgpu compositor: turns a sequence, a frame index and the already-decoded source
//! frames into one composited frame, for preview and for export.
//!
//! A leaf crate: it never decodes media or asks for frames; `dusk-engine` hands them in. It
//! also creates the wgpu device, which Slint renders with too (see docs/ARCHITECTURE.md).

mod compositor;
mod gpu;
mod yuv;

pub use compositor::{Compositor, RenderError};
pub use gpu::{Gpu, GpuError};
/// The wgpu version Dusk renders with, for crates that pass its textures on.
pub use wgpu;
pub use yuv::ToYuv;
