//! Compressing to a target file size (docs/ARCHITECTURE.md, "Target file size"): the video
//! bitrate that leaves room for the sound and the container, the picture size the bitrate
//! can carry, and the smallest file a video can become.

use std::path::Path;

use dusk_core::MediaTime;

use crate::EngineError;

/// What a compression to a target size encodes with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizePlan {
    /// The video's bitrate, in bits a second.
    pub video_bit_rate: u64,
    /// The sound's bitrate, in bits a second; 0 without sound.
    pub audio_bit_rate: usize,
    /// The picture's short side the bitrate can carry; never more than the source's.
    pub short_side: u32,
}

/// Why a target size cannot be planned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeRefusal {
    /// The target is below the floor (240p at 200 kbit/s, with 48 kbit/s of sound): the
    /// smallest file, in bytes, the video can become.
    TooSmall {
        /// The smallest size, in bytes.
        smallest: u64,
    },
    /// The source states no length to divide the size by.
    NoLength,
}

/// Share of the target left for the streams; the rest is the container's.
const STREAMS: f64 = 0.97;

/// The floor's video bitrate, in bits a second.
const FLOOR_VIDEO: u64 = 200_000;

/// The ladder from video bitrate to short side, highest first: at least this many bits a
/// second carry pictures this tall.
const LADDER: [(u64, u32); 4] = [
    (6_000_000, 1080),
    (2_500_000, 720),
    (1_000_000, 480),
    (500_000, 360),
];

/// The short side below the ladder's last step.
const SMALLEST_SIDE: u32 = 240;

/// The plan for compressing a source of `length` to `target` bytes, with sound when
/// `has_sound`.
pub fn plan_for_size(
    target: u64,
    length: MediaTime,
    has_sound: bool,
) -> Result<SizePlan, SizeRefusal> {
    if length.0 <= 0 {
        return Err(SizeRefusal::NoLength);
    }
    let seconds = length.0 as f64 / 1e6;
    // Refused by the very number it says, so a plan at the smallest size is never refused.
    let floor = FLOOR_VIDEO + if has_sound { 48_000 } else { 0 };
    let smallest = (floor as f64 * seconds / 8.0 / STREAMS).ceil() as u64;
    if target < smallest {
        return Err(SizeRefusal::TooSmall { smallest });
    }
    let streams = (target as f64 * 8.0 * STREAMS / seconds).floor() as u64;
    // Less sound for less video, decided by what the video would have with the sound before.
    let audio = if !has_sound {
        0
    } else if streams.saturating_sub(128_000) >= 1_500_000 {
        128_000
    } else if streams.saturating_sub(64_000) >= 500_000 {
        64_000
    } else {
        48_000
    };
    // At the smallest size the division can round a bit a second under the floor.
    let video = streams.saturating_sub(audio).max(FLOOR_VIDEO);
    let short_side = LADDER
        .iter()
        .find(|(rate, _)| video >= *rate)
        .map_or(SMALLEST_SIDE, |(_, side)| *side);
    Ok(SizePlan {
        video_bit_rate: video,
        audio_bit_rate: audio as usize,
        short_side,
    })
}

/// What a refused plan for the file at `path` says to the user.
pub(crate) fn refused(refusal: SizeRefusal, path: &Path) -> EngineError {
    match refusal {
        SizeRefusal::TooSmall { smallest } => EngineError::TooSmall { smallest },
        SizeRefusal::NoLength => EngineError::NoLength {
            path: path.to_path_buf(),
        },
    }
}

/// The video bitrate to make a file again with, once, when one made at `bit_rate` came out
/// at `written` bytes, more than 3% over its `target`: scaled down by the overshoot. `None`
/// when it is close enough.
pub fn corrected(bit_rate: u64, target: u64, written: u64) -> Option<u64> {
    (u128::from(written) * 100 > u128::from(target) * 103)
        .then(|| (u128::from(bit_rate) * u128::from(target) / u128::from(written)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: MediaTime = MediaTime(60_000_000);

    #[test]
    fn the_sound_and_the_container_come_off_the_target() {
        // 25 MB over a minute: 25e6 × 8 × 0.97 / 60 = 3.233 Mbit/s for both streams.
        let plan = plan_for_size(25_000_000, MINUTE, true).unwrap();
        assert_eq!(plan.audio_bit_rate, 128_000);
        assert_eq!(plan.video_bit_rate, 3_233_333 - 128_000);
        assert_eq!(plan.short_side, 720);
        let silent = plan_for_size(25_000_000, MINUTE, false).unwrap();
        assert_eq!(
            (silent.audio_bit_rate, silent.video_bit_rate),
            (0, 3_233_333)
        );
    }

    #[test]
    fn the_ladder_picks_the_picture_size() {
        let side = |megabytes: u64| {
            plan_for_size(megabytes * 1_000_000, MINUTE, false)
                .unwrap()
                .short_side
        };
        assert_eq!(side(100), 1080);
        assert_eq!(side(25), 720);
        assert_eq!(side(10), 480);
        assert_eq!(side(5), 360);
        assert_eq!(side(2), 240);
    }

    #[test]
    fn low_video_bitrates_get_less_sound() {
        // 1 Mbit/s of video gets 64 kbit/s of sound; 0.3 Mbit/s gets 48.
        assert_eq!(
            plan_for_size(9_000_000, MINUTE, true)
                .unwrap()
                .audio_bit_rate,
            64_000
        );
        assert_eq!(
            plan_for_size(3_000_000, MINUTE, true)
                .unwrap()
                .audio_bit_rate,
            48_000
        );
    }

    #[test]
    fn below_the_floor_the_smallest_size_is_offered() {
        // 248 kbit/s for a minute is 1.86 MB of streams, 1.918 MB with the container.
        assert_eq!(
            plan_for_size(1_000_000, MINUTE, true),
            Err(SizeRefusal::TooSmall {
                smallest: 1_917_526
            })
        );
        let smallest = 1_917_526;
        assert!(plan_for_size(smallest, MINUTE, true).is_ok());
        assert_eq!(
            plan_for_size(1_000_000, MINUTE, false),
            Err(SizeRefusal::TooSmall {
                smallest: 1_546_392
            })
        );
    }

    #[test]
    fn the_smallest_size_offered_is_always_taken() {
        // Rounding once made a plan at the smallest size come out a bit/s under the floor, so
        // the same smallest came back and dusq asked again forever.
        let length = MediaTime(3_594_810_106);
        let Err(SizeRefusal::TooSmall { smallest }) = plan_for_size(1_000, length, false) else {
            panic!("a kilobyte is below the floor");
        };
        assert!(plan_for_size(smallest, length, false).is_ok());
        for micros in (1_000_000..4_000_000_000).step_by(791_937) {
            let length = MediaTime(micros);
            for sound in [true, false] {
                let Err(SizeRefusal::TooSmall { smallest }) = plan_for_size(1, length, sound)
                else {
                    panic!("a byte is below the floor");
                };
                assert!(
                    plan_for_size(smallest, length, sound).is_ok(),
                    "{micros} µs"
                );
            }
        }
    }

    #[test]
    fn a_file_more_than_3_percent_over_is_made_again_smaller() {
        assert_eq!(corrected(3_000_000, 25_000_000, 25_750_000), None);
        // 10% over: the bitrate comes down by the same share.
        assert_eq!(
            corrected(3_000_000, 25_000_000, 27_500_000),
            Some(2_727_272)
        );
    }

    #[test]
    fn a_source_without_a_length_cannot_be_planned() {
        assert_eq!(
            plan_for_size(25_000_000, MediaTime(0), true),
            Err(SizeRefusal::NoLength)
        );
    }
}
