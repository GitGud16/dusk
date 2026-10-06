//! How an export is written: the file format, the picture's size, the quality and the
//! sound (docs/ARCHITECTURE.md, "Export details"). Both export paths take these: the GPU one
//! in the app and the CPU one in `dusq`.

use dusk_core::{Project, TrackKind};
use dusk_media::{AudioCodec, AudioFormat, AudioSettings, Container, Quality, VideoCodec};

/// Whether `project`'s sequence has a picture to show: a video track that is not hidden and
/// has an enabled clip on it.
pub fn has_picture(project: &Project) -> bool {
    has_clips(project, TrackKind::Video)
}

/// Whether a track of `kind` that is not muted or hidden has an enabled clip on it.
fn has_clips(project: &Project, kind: TrackKind) -> bool {
    project.sequence().tracks().iter().any(|track| {
        track.kind() == kind && !track.muted() && track.clips().iter().any(|clip| clip.enabled)
    })
}

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
/// an enabled clip on it.
pub fn has_sound(project: &Project) -> bool {
    has_clips(project, TrackKind::Audio)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{
        Command, Frame, MediaInfo, MediaKind, MediaTime, Orientation, Rational, SetClipEnabled,
        SetTrackMuted, import,
    };

    /// A project with one file imported: its video on V1 and its sound on A1, as it has them.
    fn imported(has_video: bool, has_audio: bool) -> Project {
        let rate = Rational::new(30, 1).unwrap();
        let info = MediaInfo {
            kind: if has_video {
                MediaKind::Video
            } else {
                MediaKind::Audio
            },
            duration: MediaTime(2_000_000),
            has_video,
            has_audio,
            frame_rate: has_video.then_some(rate),
            vfr: false,
            width: 1920,
            height: 1080,
            orientation: Orientation::UPRIGHT,
        };
        let mut project = Project::new(rate, (1920, 1080));
        import(&project, "clip.mp4".into(), info, Frame(0))
            .apply(&mut project)
            .unwrap();
        project
    }

    /// `project` with `command` applied.
    fn after(project: &Project, mut command: Command) -> Project {
        let mut project = project.clone();
        command.apply(&mut project).unwrap();
        project
    }

    #[test]
    fn a_picture_needs_a_shown_video_track_with_an_enabled_clip() {
        let project = imported(true, true);
        assert!(has_picture(&project));
        assert!(!has_picture(&imported(false, true)));
        let track = &project.sequence().tracks()[0];
        assert_eq!(track.kind(), TrackKind::Video);
        let hidden = SetTrackMuted::new(track.id(), true);
        assert!(!has_picture(&after(
            &project,
            Command::SetTrackMuted(hidden)
        )));
        let disabled = SetClipEnabled::new(track.clips()[0].id, false);
        assert!(!has_picture(&after(
            &project,
            Command::SetClipEnabled(disabled)
        )));
    }

    #[test]
    fn sound_needs_an_audible_track_with_an_enabled_clip() {
        let project = imported(true, true);
        assert!(has_sound(&project));
        assert!(!has_sound(&imported(true, false)));
        let track = &project.sequence().tracks()[2];
        assert_eq!(track.kind(), TrackKind::Audio);
        let muted = SetTrackMuted::new(track.id(), true);
        assert!(!has_sound(&after(&project, Command::SetTrackMuted(muted))));
        let disabled = SetClipEnabled::new(track.clips()[0].id, false);
        assert!(!has_sound(&after(
            &project,
            Command::SetClipEnabled(disabled)
        )));
    }

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
