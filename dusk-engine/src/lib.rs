//! Worker-thread orchestration: frame cache, decoder pool, playback scheduler, thumbnail
//! and waveform jobs, and the export pipeline. The only crate that moves data between the
//! leaf crates (`dusk-media`, `dusk-render`, `dusk-audio`).

#[cfg(feature = "gpu")]
mod cache;
mod compress;
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
mod settings;
mod size;
#[cfg(feature = "gpu")]
mod sound;
#[cfg(feature = "gpu")]
mod thumbnail;
mod transcode;
#[cfg(feature = "gpu")]
mod video;

pub use compress::{CompressTarget, compress_project, compress_settings};
pub use dusk_media::{
    AudioCodec, AudioFormat, Container, ENCODERS, Encoder, Quality, VideoCodec, available_encoders,
    quiet_logs,
};
#[cfg(feature = "gpu")]
pub use dusk_render::{Gpu, GpuError, wgpu};
#[cfg(feature = "gpu")]
pub use engine::{DECODER_IDLE, Engine, EngineEvent, EngineOptions, Preview};
pub use error::EngineError;
#[cfg(feature = "gpu")]
pub use export::{ExportEvent, ExportJob};
pub use info::media_info;
pub use settings::{ExportFormat, ExportSettings, export_size, has_picture, has_sound};
pub use size::{SizePlan, SizeRefusal, corrected, plan_for_size};
#[cfg(feature = "gpu")]
pub use thumbnail::{THUMBNAIL_SIDE, Thumbnail, thumbnail_of};
pub use transcode::{
    Progress, TranscodeSettings, Transcoded, decoder_threads, extract_audio, transcode,
    transcode_to_size,
};
