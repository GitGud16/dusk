//! Audio: playback speed, the playback clock and, with the `output` feature, the output device
//! fed through a lock-free ring buffer.
//!
//! A leaf crate: it never decodes media or knows about the UI; `dusk-engine` hands it PCM.

mod clock;
#[cfg(feature = "output")]
mod output;
mod speed;

pub use clock::{ClockSource, FrameCounter, PlaybackClock};
#[cfg(feature = "output")]
pub use output::{AudioError, AudioOutput};
pub use speed::SpeedResampler;
