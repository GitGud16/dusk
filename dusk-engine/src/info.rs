//! What Dusk records about a file when it is imported (docs/ARCHITECTURE.md, "Core data
//! model"), read from FFmpeg's probe.

use std::path::Path;

use dusk_core::time::{STANDARD_RATES, snap_to_standard};
use dusk_core::{MediaInfo, MediaKind, MediaTime, Rational};
use dusk_media::{ProbeInfo, StillInfo, StreamDetail, StreamKind};

use crate::EngineError;

/// Probes the local file at `path` and describes it for the project.
pub fn media_info(path: &Path) -> Result<MediaInfo, EngineError> {
    let probe = dusk_media::probe(path)?;
    let described = if dusk_media::is_still(&probe) {
        // A photo's orientation is only on its decoded picture.
        describe_still(&dusk_media::still_info(path)?)
    } else {
        describe(&probe)
    };
    described.map_err(|reason| EngineError::Unsupported {
        path: path.to_path_buf(),
        reason,
    })
}

/// The most memory decoding one still may take: half the default frame cache cap
/// (docs/ARCHITECTURE.md, "Decoder pool"), since the decode is a transient inside the cap.
pub(crate) const STILL_PEAK_LIMIT: u64 = 192 * 1024 * 1024;

/// Describes a still image, or says why Dusk cannot use it.
fn describe_still(still: &StillInfo) -> Result<MediaInfo, &'static str> {
    if still.peak_bytes > STILL_PEAK_LIMIT {
        return Err(
            "it is too large to decode within the memory Dusk keeps for pictures; save a smaller copy and import that",
        );
    }
    let (width, height) = still.orientation.apply_to_size((still.width, still.height));
    Ok(MediaInfo {
        kind: MediaKind::Still,
        duration: MediaTime(0),
        has_video: true,
        has_audio: false,
        frame_rate: None,
        vfr: false,
        width,
        height,
        orientation: still.orientation,
    })
}

/// The size, as stored, a still described by `info` is decoded at for a sequence of `frame`
/// size: enough to cover the frame whether it fits or fills, never more pixels than the photo
/// has, and no side over [`TEXTURE_LIMIT`].
// The preview and export draw stills; both come with the `gpu` feature.
#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
pub(crate) fn still_size(info: &MediaInfo, frame: (u32, u32)) -> (u32, u32) {
    let (width, height) = (f64::from(info.width.max(1)), f64::from(info.height.max(1)));
    let cover = (f64::from(frame.0) / width)
        .max(f64::from(frame.1) / height)
        .min(1.0);
    let limit = f64::from(TEXTURE_LIMIT);
    let scale = cover.min(limit / width).min(limit / height);
    let side = |length: f64| ((length * scale).round() as u32).clamp(1, TEXTURE_LIMIT);
    // Turning back to stored swaps the sides exactly when turning upright did.
    info.orientation.apply_to_size((side(width), side(height)))
}

/// The largest texture side every GPU Dusk runs on takes (wgpu's default limits, which
/// dusk-render asks for).
#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
pub(crate) const TEXTURE_LIMIT: u32 = 8192;

/// Describes a probed file, or says why Dusk cannot use it.
fn describe(probe: &ProbeInfo) -> Result<MediaInfo, &'static str> {
    // FFmpeg's single-image demuxers; still images arrive in M2.
    if probe.format == "image2" || probe.format.ends_with("_pipe") {
        return Err("still images arrive in a later version of Dusk");
    }
    // Cover art is a picture stream too, but not video.
    let video = probe.streams.iter().find_map(|stream| match stream.detail {
        StreamDetail::Video {
            width,
            height,
            frame_rate,
            base_frame_rate,
            cover_art: false,
            orientation,
        } => Some((width, height, frame_rate, base_frame_rate, orientation)),
        _ => None,
    });
    let has_audio = probe
        .streams
        .iter()
        .any(|stream| stream.kind == StreamKind::Audio);
    if video.is_none() && !has_audio {
        return Err("it has neither video nor audio");
    }
    let duration = probe
        .duration_us
        .filter(|us| *us > 0)
        .ok_or("FFmpeg cannot tell how long it is; convert it to MP4 and try again")?;
    let (width, height, frame_rate, vfr, orientation) = match video {
        None => (0, 0, None, false, dusk_core::Orientation::UPRIGHT),
        Some((width, height, average, base, orientation)) => {
            let rate = |rate: Option<(i32, i32)>| {
                let (num, den) = rate?;
                Rational::new(u32::try_from(num).ok()?, u32::try_from(den).ok()?)
            };
            let (average, base) = (rate(average), rate(base));
            // A variable rate shows as an average that differs from the base rate FFmpeg
            // guesses from the timestamps; it snaps to the nearest standard rate.
            let vfr = average.is_none() || average != base;
            let rate = match average.or(base) {
                Some(rate) if !vfr => rate,
                Some(rate) => snap_to_standard(rate),
                // No rate at all: 30 fps, the default sequence rate.
                None => STANDARD_RATES[4],
            };
            // Width and height are the upright picture's.
            let (width, height) = orientation.apply_to_size((width, height));
            (width, height, Some(rate), vfr, orientation)
        }
    };
    Ok(MediaInfo {
        kind: if video.is_some() {
            MediaKind::Video
        } else {
            MediaKind::Audio
        },
        duration: MediaTime(duration),
        has_video: video.is_some(),
        has_audio,
        frame_rate,
        vfr,
        width,
        height,
        orientation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_media::StreamSummary;

    fn rate(num: u32, den: u32) -> Rational {
        Rational::new(num, den).unwrap()
    }

    fn video(width: u32, height: u32, average: (i32, i32), base: (i32, i32)) -> StreamSummary {
        StreamSummary {
            index: 0,
            kind: StreamKind::Video,
            codec: "h264".to_owned(),
            detail: StreamDetail::Video {
                width,
                height,
                frame_rate: Some(average),
                base_frame_rate: Some(base),
                cover_art: false,
                orientation: dusk_core::Orientation::UPRIGHT,
            },
        }
    }

    #[test]
    fn a_turned_video_is_described_upright() {
        let mut phone = video(1920, 1080, (30, 1), (30, 1));
        if let StreamDetail::Video { orientation, .. } = &mut phone.detail {
            *orientation = dusk_core::Orientation::new(1, false);
        }
        let info = describe(&file("mov,mp4,m4a,3gp,3g2,mj2", vec![phone])).unwrap();
        assert_eq!((info.width, info.height), (1080, 1920));
        assert_eq!(info.orientation, dusk_core::Orientation::new(1, false));
    }

    fn audio() -> StreamSummary {
        StreamSummary {
            index: 1,
            kind: StreamKind::Audio,
            codec: "aac".to_owned(),
            detail: StreamDetail::Audio {
                sample_rate: 48_000,
                channels: 2,
            },
        }
    }

    fn cover_art() -> StreamSummary {
        StreamSummary {
            index: 2,
            kind: StreamKind::Video,
            codec: "mjpeg".to_owned(),
            detail: StreamDetail::Video {
                width: 600,
                height: 600,
                frame_rate: None,
                base_frame_rate: None,
                cover_art: true,
                orientation: dusk_core::Orientation::UPRIGHT,
            },
        }
    }

    fn file(format: &str, streams: Vec<StreamSummary>) -> ProbeInfo {
        ProbeInfo {
            format: format.to_owned(),
            duration_us: Some(13_680_000),
            streams,
        }
    }

    #[test]
    fn a_constant_rate_video_keeps_its_own_rate() {
        let info = describe(&file(
            "mov,mp4,m4a,3gp,3g2,mj2",
            vec![video(1920, 1080, (25, 1), (25, 1)), audio()],
        ))
        .unwrap();
        assert_eq!(
            info,
            MediaInfo {
                kind: MediaKind::Video,
                duration: MediaTime(13_680_000),
                has_video: true,
                has_audio: true,
                frame_rate: Some(rate(25, 1)),
                vfr: false,
                width: 1920,
                height: 1080,
                orientation: dusk_core::Orientation::UPRIGHT,
            }
        );
    }

    #[test]
    fn a_phone_recording_snaps_to_a_standard_rate() {
        // The phone clip from the M1 checks: 393 frames in 13.6 s, base rate 30.
        let info = describe(&file(
            "mov,mp4,m4a,3gp,3g2,mj2",
            vec![audio(), video(720, 1280, (35_370_000, 1_224_653), (30, 1))],
        ))
        .unwrap();
        assert!(info.vfr);
        assert_eq!(info.frame_rate, Some(rate(30000, 1001)));
        assert_eq!((info.width, info.height), (720, 1280));
    }

    #[test]
    fn music_with_cover_art_is_audio_only() {
        let info = describe(&file("mp3", vec![audio(), cover_art()])).unwrap();
        assert_eq!(info.kind, MediaKind::Audio);
        assert!(!info.has_video && info.has_audio);
        assert_eq!(info.frame_rate, None);
        assert_eq!((info.width, info.height), (0, 0));
    }

    #[test]
    fn files_dusk_cannot_use_yet_are_refused() {
        assert!(describe(&file("srt", vec![])).is_err());
        let mut unknown_length = file("mpegts", vec![video(640, 480, (25, 1), (25, 1))]);
        unknown_length.duration_us = None;
        assert!(describe(&unknown_length).is_err());
    }

    fn photo(width: u32, height: u32, orientation: dusk_core::Orientation) -> MediaInfo {
        describe_still(&StillInfo {
            width,
            height,
            orientation,
            peak_bytes: u64::from(width) * u64::from(height) * 3 / 2,
        })
        .unwrap()
    }

    #[test]
    fn a_photo_is_a_still_described_upright() {
        let info = photo(4032, 3024, dusk_core::Orientation::new(1, false));
        assert_eq!(info.kind, MediaKind::Still);
        assert_eq!((info.width, info.height), (3024, 4032));
        assert_eq!(info.orientation, dusk_core::Orientation::new(1, false));
        assert!(info.has_video && !info.has_audio);
        assert_eq!((info.duration, info.frame_rate), (MediaTime(0), None));
    }

    #[test]
    fn a_photo_too_large_to_decode_is_refused() {
        let huge = StillInfo {
            width: 12_000,
            height: 9_000,
            orientation: dusk_core::Orientation::UPRIGHT,
            // 16-bit RGBA.
            peak_bytes: 12_000 * 9_000 * 8,
        };
        assert!(describe_still(&huge).is_err());
    }

    #[test]
    fn a_still_is_decoded_just_large_enough_to_cover_the_frame() {
        let upright = dusk_core::Orientation::UPRIGHT;
        assert_eq!(
            still_size(&photo(4032, 3024, upright), (1920, 1080)),
            (1920, 1440)
        );
        // Never larger than the photo.
        assert_eq!(
            still_size(&photo(640, 480, upright), (1920, 1080)),
            (640, 480)
        );
        // A photo turned upright: the size is worked out upright, then given as stored.
        let turned = photo(4032, 3024, dusk_core::Orientation::new(1, false));
        assert_eq!(still_size(&turned, (1920, 1080)), (2560, 1920));
        // A panorama is kept within the largest texture.
        assert_eq!(
            still_size(&photo(20_000, 3_000, upright), (8192, 8192)),
            (8192, 1229)
        );
    }

    #[test]
    fn a_photo_file_is_described() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/photo-turned.jpg");
        let info = media_info(&path).unwrap();
        assert_eq!(info.kind, MediaKind::Still);
        assert_eq!((info.width, info.height), (240, 320));
        assert_eq!(info.orientation, dusk_core::Orientation::new(1, false));
    }

    #[test]
    fn the_sample_file_is_described() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-h264-aac.mp4");
        let info = media_info(&sample).unwrap();
        assert_eq!(info.kind, MediaKind::Video);
        assert_eq!(info.duration, MediaTime(1_000_000));
        assert_eq!(info.frame_rate, Some(rate(30, 1)));
        assert!(info.has_audio && !info.vfr);
    }
}
