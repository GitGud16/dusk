//! The compress tool's choices (docs/ARCHITECTURE.md, "Compress tool paths"): a video aimed
//! at a size in whole megabytes or at a quality, and what it will come out as.

use dusk_core::MediaInfo;
use dusk_engine::{CompressTarget, SizeRefusal, export_size, plan_for_size};

/// What the compress dialog shows and has chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct CompressChoices {
    /// The video.
    pub info: MediaInfo,
    /// Its file's size, in bytes.
    pub file_bytes: u64,
    /// Aiming at a size, rather than a quality.
    pub by_size: bool,
    /// The size aimed at, in whole megabytes.
    pub megabytes: u32,
    /// The Quality slider, 0 to 100.
    pub level: u8,
}

/// What a compression will make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A file of `width` by `height` with video at `video` and sound at `sound` bits a
    /// second (0 without sound).
    Planned {
        width: u32,
        height: u32,
        video: u64,
        sound: usize,
    },
    /// The video's own size at a quality; the file's size follows from the picture.
    AtQuality { width: u32, height: u32 },
    /// The size is below the smallest the video can become, which is `megabytes` rounded up.
    TooSmall { megabytes: u32 },
    /// The video does not say how long it is, so it cannot be made to a size.
    NoLength,
}

/// The size the dialog starts at: 25 MB, which mail and chat apps take, or half the video
/// when it is smaller than that already.
const START: u32 = 25;

impl CompressChoices {
    /// The choices for a video described by `info` whose file holds `file_bytes`.
    pub fn new(info: MediaInfo, file_bytes: u64) -> CompressChoices {
        let file_megabytes = file_bytes / 1_000_000;
        let megabytes = if file_megabytes > u64::from(START) {
            START
        } else {
            u32::try_from(file_megabytes / 2).unwrap_or(START).max(1)
        };
        CompressChoices {
            info,
            file_bytes,
            by_size: true,
            megabytes,
            level: 60,
        }
    }

    /// What the engine aims at.
    pub fn target(&self) -> CompressTarget {
        if self.by_size {
            CompressTarget::Size(self.bytes())
        } else {
            CompressTarget::Quality(self.level.min(100))
        }
    }

    /// The size aimed at, in bytes.
    fn bytes(&self) -> u64 {
        u64::from(self.megabytes.max(1)) * 1_000_000
    }

    /// What it will come out as.
    pub fn outcome(&self) -> Outcome {
        let size = (self.info.width, self.info.height);
        if !self.by_size {
            return Outcome::AtQuality {
                width: size.0,
                height: size.1,
            };
        }
        match plan_for_size(self.bytes(), self.info.duration, self.info.has_audio) {
            Ok(plan) => {
                let (width, height) = export_size(size, Some(plan.short_side));
                Outcome::Planned {
                    width,
                    height,
                    video: plan.video_bit_rate,
                    sound: plan.audio_bit_rate,
                }
            }
            Err(SizeRefusal::TooSmall { smallest }) => Outcome::TooSmall {
                megabytes: u32::try_from(smallest.div_ceil(1_000_000)).unwrap_or(u32::MAX),
            },
            Err(SizeRefusal::NoLength) => Outcome::NoLength,
        }
    }

    /// Whether Compress can go ahead.
    pub fn ready(&self) -> bool {
        matches!(
            self.outcome(),
            Outcome::Planned { .. } | Outcome::AtQuality { .. }
        )
    }

    /// Whether the size aimed at is no smaller than the video's file already.
    pub fn not_smaller(&self) -> bool {
        self.by_size && self.bytes() >= self.file_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{MediaKind, MediaTime, Orientation, Rational};

    /// A minute of portrait phone video with sound, 200 MB.
    fn phone() -> CompressChoices {
        let info = MediaInfo {
            kind: MediaKind::Video,
            duration: MediaTime(60_000_000),
            has_video: true,
            has_audio: true,
            frame_rate: Some(Rational::new(30, 1).unwrap()),
            vfr: true,
            width: 1080,
            height: 1920,
            orientation: Orientation::new(1, false),
        };
        CompressChoices::new(info, 200_000_000)
    }

    #[test]
    fn it_starts_at_25_megabytes_or_half_a_smaller_video() {
        let phone = phone();
        assert!(phone.by_size);
        assert_eq!((phone.megabytes, phone.level), (25, 60));
        let small = CompressChoices::new(phone.info.clone(), 9_000_000);
        assert_eq!(small.megabytes, 4);
        let tiny = CompressChoices::new(phone.info, 1_000_000);
        assert_eq!(tiny.megabytes, 1);
    }

    #[test]
    fn a_size_plans_the_picture_and_the_bitrates() {
        let phone = phone();
        assert_eq!(phone.target(), CompressTarget::Size(25_000_000));
        // 25 MB over a minute: 720p, portrait.
        assert_eq!(
            phone.outcome(),
            Outcome::Planned {
                width: 720,
                height: 1280,
                video: 3_105_333,
                sound: 128_000,
            }
        );
        assert!(phone.ready());
    }

    #[test]
    fn a_quality_keeps_the_video_size() {
        let phone = CompressChoices {
            by_size: false,
            level: 80,
            ..phone()
        };
        assert_eq!(phone.target(), CompressTarget::Quality(80));
        assert_eq!(
            phone.outcome(),
            Outcome::AtQuality {
                width: 1080,
                height: 1920
            }
        );
        assert!(phone.ready());
    }

    #[test]
    fn a_size_too_small_says_the_smallest_and_cannot_go_ahead() {
        let phone = CompressChoices {
            megabytes: 1,
            ..phone()
        };
        // 1.92 MB at the floor, rounded up.
        assert_eq!(phone.outcome(), Outcome::TooSmall { megabytes: 2 });
        assert!(!phone.ready());
    }

    #[test]
    fn a_size_needs_the_videos_length() {
        let mut phone = phone();
        phone.info.duration = MediaTime(0);
        assert_eq!(phone.outcome(), Outcome::NoLength);
        assert!(!phone.ready());
        phone.by_size = false;
        assert!(phone.ready());
    }

    #[test]
    fn a_size_no_smaller_than_the_video_is_pointed_out() {
        let phone = phone();
        assert!(!phone.not_smaller());
        let big = CompressChoices {
            megabytes: 200,
            ..phone
        };
        assert!(big.not_smaller());
        assert!(big.ready());
    }
}
