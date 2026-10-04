//! Timeline time and media time, and the one place where they are converted
//! (docs/ARCHITECTURE.md, "Time units"). Every conversion rounds to the nearest unit, with
//! halves rounded away from zero.

use std::ops::{Add, Sub};

/// A position or length on the timeline, counted in frames at the sequence frame rate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Frame(pub i64);

/// A time in a source file, in microseconds (FFmpeg's `AV_TIME_BASE`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MediaTime(pub i64);

impl MediaTime {
    /// One second.
    pub const SECOND: MediaTime = MediaTime(1_000_000);
}

impl Add for Frame {
    type Output = Frame;
    fn add(self, other: Frame) -> Frame {
        Frame(self.0 + other.0)
    }
}

impl Sub for Frame {
    type Output = Frame;
    fn sub(self, other: Frame) -> Frame {
        Frame(self.0 - other.0)
    }
}

impl Add for MediaTime {
    type Output = MediaTime;
    fn add(self, other: MediaTime) -> MediaTime {
        MediaTime(self.0 + other.0)
    }
}

impl Sub for MediaTime {
    type Output = MediaTime;
    fn sub(self, other: MediaTime) -> MediaTime {
        MediaTime(self.0 - other.0)
    }
}

/// A frame rate as an exact fraction in lowest terms, such as 30000/1001 for 29.97 fps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rational {
    num: u32,
    den: u32,
}

impl Rational {
    /// `num / den` in lowest terms, or `None` if either is zero.
    pub fn new(num: u32, den: u32) -> Option<Rational> {
        if num == 0 || den == 0 {
            return None;
        }
        let divisor = gcd(num, den);
        Some(Rational {
            num: num / divisor,
            den: den / divisor,
        })
    }

    /// The numerator.
    pub fn num(self) -> u32 {
        self.num
    }

    /// The denominator.
    pub fn den(self) -> u32 {
        self.den
    }
}

/// The rates variable-frame-rate sources snap to (docs/ARCHITECTURE.md, "Sequence settings"):
/// 23.976, 24, 25, 29.97, 30, 50, 59.94 and 60.
pub const STANDARD_RATES: [Rational; 8] = [
    Rational {
        num: 24000,
        den: 1001,
    },
    Rational { num: 24, den: 1 },
    Rational { num: 25, den: 1 },
    Rational {
        num: 30000,
        den: 1001,
    },
    Rational { num: 30, den: 1 },
    Rational { num: 50, den: 1 },
    Rational {
        num: 60000,
        den: 1001,
    },
    Rational { num: 60, den: 1 },
];

/// The standard rate nearest to `rate`; on a tie, the lower one.
pub fn snap_to_standard(rate: Rational) -> Rational {
    // |rate - standard| = |n·d' - n'·d| / (d·d'); d is the same for every candidate, so
    // compare |n·d' - n'·d| / d' between candidates, exactly, by cross-multiplying.
    let gap = |standard: &Rational| {
        let (n, d) = (i128::from(rate.num), i128::from(rate.den));
        let (n2, d2) = (i128::from(standard.num), i128::from(standard.den));
        ((n * d2 - n2 * d).abs(), d2)
    };
    let mut best = STANDARD_RATES[0];
    let mut best_gap = gap(&best);
    for standard in &STANDARD_RATES[1..] {
        let (over, under) = gap(standard);
        if over * best_gap.1 < best_gap.0 * under {
            best = *standard;
            best_gap = (over, under);
        }
    }
    best
}

/// The media time at which timeline frame `frame` starts, at `rate` frames per second.
pub fn frame_to_media(frame: Frame, rate: Rational) -> MediaTime {
    MediaTime(saturate(div_round(
        i128::from(frame.0) * MICROS * i128::from(rate.den),
        i128::from(rate.num),
    )))
}

/// The timeline frame nearest to media time `time`, at `rate` frames per second.
pub fn media_to_frame(time: MediaTime, rate: Rational) -> Frame {
    Frame(saturate(div_round(
        i128::from(time.0) * i128::from(rate.num),
        MICROS * i128::from(rate.den),
    )))
}

/// How many timeline frames `source` of media plays for at `speed`: never less than one
/// (docs/ARCHITECTURE.md: `max(1, round(((source_out - source_in) / speed) × frame_rate))`).
pub fn length_for(source: MediaTime, speed: f64, rate: Rational) -> Frame {
    // Exact in f64 for any realistic duration (below 2^53 µs × rate); see the tests.
    let frames = source.0 as f64 / speed * f64::from(rate.num) / (1e6 * f64::from(rate.den));
    Frame((frames.round() as i64).max(1))
}

/// How much source media `length` timeline frames play at `speed`.
pub fn source_span(length: Frame, speed: f64, rate: Rational) -> MediaTime {
    MediaTime((frame_to_media(length, rate).0 as f64 * speed).round() as i64)
}

const MICROS: i128 = 1_000_000;

/// `n / d` rounded to the nearest integer, halves away from zero; `d` must be positive.
fn div_round(n: i128, d: i128) -> i128 {
    let (quotient, remainder) = (n / d, n % d);
    if 2 * remainder.abs() >= d {
        quotient + n.signum()
    } else {
        quotient
    }
}

fn saturate(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(num: u32, den: u32) -> Rational {
        Rational::new(num, den).unwrap()
    }

    #[test]
    fn a_rate_is_kept_in_lowest_terms() {
        assert_eq!(rate(60, 2), rate(30, 1));
        assert_eq!((rate(60, 2).num(), rate(60, 2).den()), (30, 1));
        assert_eq!(
            (rate(30000, 1001).num(), rate(30000, 1001).den()),
            (30000, 1001)
        );
    }

    #[test]
    fn a_variable_rate_snaps_to_the_nearest_standard_rate() {
        // An Android phone clip: 393 frames in 13.6 s.
        assert_eq!(
            snap_to_standard(rate(35_370_000, 1_224_653)),
            rate(30000, 1001)
        );
        assert_eq!(snap_to_standard(rate(30, 1)), rate(30, 1));
        assert_eq!(snap_to_standard(rate(5960, 100)), rate(60000, 1001));
        assert_eq!(snap_to_standard(rate(47, 1)), rate(50, 1));
        assert_eq!(snap_to_standard(rate(15, 1)), rate(24000, 1001));
        assert_eq!(snap_to_standard(rate(120, 1)), rate(60, 1));
        // 24.5 is as far from 24 as from 25: the lower one wins.
        assert_eq!(snap_to_standard(rate(49, 2)), rate(24, 1));
    }

    #[test]
    fn a_rate_with_a_zero_is_refused() {
        assert_eq!(Rational::new(0, 1), None);
        assert_eq!(Rational::new(30, 0), None);
    }

    #[test]
    fn frames_convert_to_media_time_rounded_to_the_microsecond() {
        assert_eq!(frame_to_media(Frame(30), rate(30, 1)), MediaTime::SECOND);
        // 33 333.3 µs
        assert_eq!(frame_to_media(Frame(1), rate(30, 1)), MediaTime(33_333));
        // 33 366.7 µs
        assert_eq!(
            frame_to_media(Frame(1), rate(30000, 1001)),
            MediaTime(33_367)
        );
        assert_eq!(
            frame_to_media(Frame(30000), rate(30000, 1001)),
            MediaTime(1_001_000_000)
        );
    }

    #[test]
    fn media_time_converts_to_the_nearest_frame() {
        let pal = rate(25, 1); // 40 000 µs per frame
        assert_eq!(media_to_frame(MediaTime(19_999), pal), Frame(0));
        assert_eq!(media_to_frame(MediaTime(20_000), pal), Frame(1)); // a half rounds up
        assert_eq!(media_to_frame(MediaTime(59_999), pal), Frame(1));
        assert_eq!(media_to_frame(MediaTime(-20_000), pal), Frame(-1)); // and away from zero
    }

    #[test]
    fn frames_survive_a_round_trip_through_media_time() {
        for r in [
            rate(24000, 1001),
            rate(25, 1),
            rate(30000, 1001),
            rate(60, 1),
        ] {
            for f in 0..5_000 {
                assert_eq!(
                    media_to_frame(frame_to_media(Frame(f), r), r),
                    Frame(f),
                    "{r:?}"
                );
            }
        }
    }

    #[test]
    fn length_follows_the_source_duration_and_speed() {
        let ntsc = rate(30000, 1001);
        assert_eq!(length_for(MediaTime::SECOND, 1.0, rate(30, 1)), Frame(30));
        assert_eq!(
            length_for(MediaTime(2_000_000), 2.0, rate(30, 1)),
            Frame(30)
        );
        assert_eq!(
            length_for(MediaTime(1_000_000), 0.5, rate(30, 1)),
            Frame(60)
        );
        assert_eq!(length_for(MediaTime(1_001_000), 1.0, ntsc), Frame(30));
    }

    #[test]
    fn length_is_never_below_one_frame() {
        assert_eq!(length_for(MediaTime(1), 1.0, rate(30, 1)), Frame(1));
        assert_eq!(length_for(MediaTime(10_000), 32.0, rate(30, 1)), Frame(1));
    }

    #[test]
    fn at_normal_speed_length_matches_the_frame_conversion() {
        let ntsc = rate(30000, 1001);
        for us in (40_000..4_000_000).step_by(16_667) {
            assert_eq!(
                length_for(MediaTime(us), 1.0, ntsc),
                media_to_frame(MediaTime(us), ntsc),
                "{us} µs"
            );
        }
    }

    #[test]
    fn source_span_is_the_inverse_of_length() {
        let ntsc = rate(30000, 1001);
        assert_eq!(source_span(Frame(30), 1.0, rate(30, 1)), MediaTime::SECOND);
        assert_eq!(
            source_span(Frame(30), 2.0, rate(30, 1)),
            MediaTime(2_000_000)
        );
        for speed in [0.25, 1.0, 1.5, 4.0] {
            for f in 1..2_000 {
                let span = source_span(Frame(f), speed, ntsc);
                assert_eq!(
                    length_for(span, speed, ntsc),
                    Frame(f),
                    "{f} frames at {speed}x"
                );
            }
        }
    }
}
