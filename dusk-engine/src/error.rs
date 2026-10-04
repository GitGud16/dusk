use std::path::PathBuf;

use dusk_media::MediaError;

/// Why the engine could not do what it was asked. Messages say what happened and what the
/// user can do.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// Reading, decoding or writing media failed.
    #[error(transparent)]
    Media(#[from] MediaError),
    /// The file is not something Dusk can import yet.
    #[error("{} cannot be imported: {reason}", path.display())]
    Unsupported {
        /// The file.
        path: PathBuf,
        /// What is missing or not supported yet.
        reason: &'static str,
    },
    /// Drawing a frame failed.
    #[cfg(feature = "gpu")]
    #[error(transparent)]
    Render(#[from] dusk_render::RenderError),
    /// The audio device failed.
    #[cfg(feature = "gpu")]
    #[error(transparent)]
    Audio(#[from] dusk_audio::AudioError),
    /// A worker thread could not start.
    #[error("Dusk could not start a worker thread ({0}); close some programs and try again")]
    Thread(std::io::Error),
}
