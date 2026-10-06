//! The compress tool's one-clip export (docs/ARCHITECTURE.md, "Compress tool paths"): a video
//! file alone on a sequence of its own, exported through the normal pipeline at a quality or
//! to a size, without touching any project.

use std::path::{Path, PathBuf};

use dusk_core::time::STANDARD_RATES;
use dusk_core::{Frame, MediaInfo, Project, import};
use dusk_media::{AudioCodec, Container, MediaError, Quality, VideoCodec};

use crate::EngineError;
use crate::settings::{ExportFormat, ExportSettings};
use crate::size::{plan_for_size, refused};

/// The sound of a compressed video at a quality (docs/ARCHITECTURE.md, "Target file size").
const SOUND: usize = 128_000;

/// What the compress tool aims at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CompressTarget {
    /// A file of about this many bytes (docs/ARCHITECTURE.md, "Target file size").
    Size(u64),
    /// The Quality slider, 0 to 100, at the video's own size.
    Quality(u8),
}

/// The project the compress tool exports: the video at `source`, described by `info`, alone
/// on a sequence of its own upright size and frame rate.
pub fn compress_project(source: PathBuf, info: MediaInfo) -> Result<Project, EngineError> {
    if !info.has_video {
        return Err(MediaError::NoVideo { path: source }.into());
    }
    // A video always has a rate; 30 a second, the app's default, stands in should a probe
    // have none.
    let rate = info.frame_rate.unwrap_or(STANDARD_RATES[4]);
    let mut project = Project::new(rate, (info.width, info.height));
    import(&project, source.clone(), info, Frame(0))
        .apply(&mut project)
        .map_err(|_| EngineError::Unsupported {
            path: source,
            reason: "it could not be placed on a timeline; convert it to MP4 and try again",
        })?;
    Ok(project)
}

/// The export settings for compressing the video at `source`, described by `info`, to
/// `target`: MP4 with H.264 and AAC, which every player takes. With them, for a size, the
/// size to make the file again to when it comes out too far over.
pub fn compress_settings(
    source: &Path,
    info: &MediaInfo,
    target: CompressTarget,
) -> Result<(ExportSettings, Option<u64>), EngineError> {
    let format = ExportFormat::Video {
        container: Container::Mp4,
        codec: VideoCodec::H264,
        audio: AudioCodec::Aac,
    };
    match target {
        CompressTarget::Quality(level) => Ok((
            ExportSettings {
                format,
                short_side: None,
                quality: Quality::Level(level.min(100)),
                audio_bit_rate: Some(SOUND),
            },
            None,
        )),
        CompressTarget::Size(bytes) => {
            let plan = plan_for_size(bytes, info.duration, info.has_audio)
                .map_err(|refusal| refused(refusal, source))?;
            let settings = ExportSettings {
                format,
                short_side: Some(plan.short_side),
                quality: Quality::Bitrate(plan.video_bit_rate),
                audio_bit_rate: (plan.audio_bit_rate > 0).then_some(plan.audio_bit_rate),
            };
            Ok((settings, Some(bytes)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{MediaKind, MediaTime, Orientation, Rational, TrackKind};

    /// A minute of portrait phone video with sound, stored turned a quarter.
    fn phone() -> MediaInfo {
        MediaInfo {
            kind: MediaKind::Video,
            duration: MediaTime(60_000_000),
            has_video: true,
            has_audio: true,
            frame_rate: Some(Rational::new(30, 1).unwrap()),
            vfr: true,
            width: 1080,
            height: 1920,
            orientation: Orientation::new(1, false),
        }
    }

    #[test]
    fn the_video_is_alone_on_a_sequence_of_its_own() {
        let project = compress_project("phone.mp4".into(), phone()).unwrap();
        let sequence = project.sequence();
        assert_eq!(sequence.resolution(), (1080, 1920));
        assert_eq!(sequence.frame_rate(), Rational::new(30, 1).unwrap());
        let clips: Vec<_> = sequence
            .tracks()
            .iter()
            .flat_map(|track| track.clips().iter().map(move |clip| (track.kind(), clip)))
            .collect();
        assert_eq!(clips.len(), 2);
        let kinds: Vec<TrackKind> = clips.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(kinds, [TrackKind::Video, TrackKind::Audio]);
        for (_, clip) in &clips {
            assert_eq!((clip.position, clip.length), (Frame(0), Frame(1800)));
        }
        assert!(clips[0].1.link.is_some() && clips[0].1.link == clips[1].1.link);
    }

    #[test]
    fn a_file_without_video_is_refused() {
        let info = MediaInfo {
            kind: MediaKind::Audio,
            has_video: false,
            frame_rate: None,
            width: 0,
            height: 0,
            ..phone()
        };
        assert!(matches!(
            compress_project("song.mp3".into(), info),
            Err(EngineError::Media(MediaError::NoVideo { .. }))
        ));
    }

    #[test]
    fn a_size_sets_the_bitrates_and_the_short_side() {
        let (settings, again) = compress_settings(
            Path::new("phone.mp4"),
            &phone(),
            CompressTarget::Size(25_000_000),
        )
        .unwrap();
        assert_eq!(
            settings.format,
            ExportFormat::Video {
                container: Container::Mp4,
                codec: VideoCodec::H264,
                audio: AudioCodec::Aac,
            }
        );
        assert_eq!(settings.quality, Quality::Bitrate(3_105_333));
        assert_eq!(settings.audio_bit_rate, Some(128_000));
        assert_eq!(settings.short_side, Some(720));
        assert_eq!(again, Some(25_000_000));
    }

    #[test]
    fn a_quality_keeps_the_video_size() {
        let (settings, again) = compress_settings(
            Path::new("phone.mp4"),
            &phone(),
            CompressTarget::Quality(60),
        )
        .unwrap();
        assert_eq!(settings.quality, Quality::Level(60));
        assert_eq!(settings.short_side, None);
        assert_eq!(settings.audio_bit_rate, Some(128_000));
        assert_eq!(again, None);
    }

    #[test]
    fn a_size_below_the_floor_is_refused_with_the_smallest() {
        let refused = compress_settings(
            Path::new("phone.mp4"),
            &phone(),
            CompressTarget::Size(1_000_000),
        );
        assert!(matches!(
            refused,
            Err(EngineError::TooSmall {
                smallest: 1_917_526
            })
        ));
        let silent = MediaInfo {
            duration: MediaTime(0),
            ..phone()
        };
        assert!(matches!(
            compress_settings(
                Path::new("phone.mp4"),
                &silent,
                CompressTarget::Size(1_000_000)
            ),
            Err(EngineError::NoLength { .. })
        ));
    }
}
