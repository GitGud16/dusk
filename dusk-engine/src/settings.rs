//! How an export is written: the file format, the picture's size, the quality and the
//! sound (docs/ARCHITECTURE.md, "Export details"). Both export paths take these: the GPU one
//! in the app and the CPU one in `dusq`.

use dusk_core::{Project, TrackKind};
use dusk_media::{AudioCodec, AudioFormat, AudioSettings, Container, Quality, VideoCodec};

/// The sample rate exported sound is mixed and written at.
#[cfg_attr(
    not(feature = "gpu"),
    expect(dead_code, reason = "dusq's CPU path takes it up in M4's fifth step")
)]
pub(crate) const AUDIO_RATE: u32 = 48_000;

/// What an export writes (docs/ARCHITECTURE.md, "Export details").
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExportFormat {
    /// A video file, with the sequence's sound in it when it has any.
    Video {
        container: Container,
        codec: VideoCodec,
        audio: AudioCodec,
    },
    /// The sequence's sound alone.
    Sound(AudioFormat),
}

/// How an export is written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportSettings {
    pub format: ExportFormat,
    /// The picture's short side in pixels, such as 1080, 720 or 480; `None` for the
    /// sequence's own size. A preset never enlarges the picture.
    pub short_side: Option<u32>,
    pub quality: Quality,
    /// Bits a second of sound; `None` for the codec's usual rate.
    pub audio_bit_rate: Option<usize>,
}

impl Default for ExportSettings {
    /// H.264 with AAC sound in MP4, at the sequence's size and the High preset.
    fn default() -> ExportSettings {
        ExportSettings {
            format: ExportFormat::Video {
                container: Container::Mp4,
                codec: VideoCodec::H264,
                audio: AudioCodec::Aac,
            },
            short_side: None,
            quality: Quality::HIGH,
            audio_bit_rate: None,
        }
    }
}

impl ExportSettings {
    /// The file name extension the format takes, without the dot.
    pub fn extension(&self) -> &'static str {
        match self.format {
            ExportFormat::Video { container, .. } => container.extension(),
            ExportFormat::Sound(format) => format.extension(),
        }
    }

    /// The sound settings for `codec`, at the sample rate exports mix at.
    #[cfg_attr(
        not(feature = "gpu"),
        expect(dead_code, reason = "dusq's CPU path takes it up in M4's fifth step")
    )]
    pub(crate) fn audio(&self, codec: AudioCodec) -> AudioSettings {
        let mut audio = AudioSettings::of(codec, AUDIO_RATE);
        if let Some(bit_rate) = self.audio_bit_rate {
            audio.bit_rate = bit_rate;
        }
        audio
    }
}

/// The picture size a sequence of `size` exports at with its short side at `short_side`:
/// the same shape, even on both sides, and never larger than the sequence.
pub fn export_size(size: (u32, u32), short_side: Option<u32>) -> (u32, u32) {
    let short = size.0.min(size.1).max(1);
    let Some(wanted) = short_side.filter(|wanted| *wanted < short) else {
        return size;
    };
    let scale = f64::from(wanted) / f64::from(short);
    let even = |length: u32| ((f64::from(length) * scale / 2.0).round() as u32 * 2).max(2);
    (even(size.0), even(size.1))
}

/// Whether `project`'s sequence has sound to hear: an audio track that is not muted and has
/// a clip on it.
#[cfg_attr(
    not(feature = "gpu"),
    expect(dead_code, reason = "dusq's CPU path takes it up in M4's fifth step")
)]
pub(crate) fn has_sound(project: &Project) -> bool {
    project.sequence().tracks().iter().any(|track| {
        track.kind() == TrackKind::Audio && !track.muted() && !track.clips().is_empty()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preset_sets_the_short_side_keeping_the_shape() {
        assert_eq!(export_size((1920, 1080), Some(720)), (1280, 720));
        // A portrait sequence at 1080p exports as 1080x1920 (docs/ARCHITECTURE.md).
        assert_eq!(export_size((2160, 3840), Some(1080)), (1080, 1920));
        // 1920 x 480/1080 is 853.3, which rounds to the nearest even width.
        assert_eq!(export_size((1920, 1080), Some(480)), (854, 480));
    }

    #[test]
    fn a_preset_never_enlarges_the_picture() {
        assert_eq!(export_size((1280, 720), Some(1080)), (1280, 720));
        assert_eq!(export_size((1280, 720), Some(720)), (1280, 720));
        assert_eq!(export_size((1280, 720), None), (1280, 720));
    }

    #[test]
    fn the_extension_follows_the_format() {
        assert_eq!(ExportSettings::default().extension(), "mp4");
        let sound = ExportSettings {
            format: ExportFormat::Sound(AudioFormat::Opus),
            ..ExportSettings::default()
        };
        assert_eq!(sound.extension(), "opus");
    }
}
