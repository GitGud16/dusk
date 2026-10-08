//! How loud a clip plays from moment to moment: its volume, faded in from silence at its
//! start and out to silence at its end (docs/FEATURES.md, "Audio").

/// A clip's gain over its length. Times are microseconds from the clip's start on the
/// timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Envelope {
    gain: f32,
    fade_in: i64,
    fade_out: i64,
    length: i64,
}

impl Envelope {
    /// A clip `length` long at `volume_db` decibels, fading in over its first `fade_in` and
    /// out over its last `fade_out`.
    pub fn new(volume_db: f32, fade_in: i64, fade_out: i64, length: i64) -> Envelope {
        Envelope {
            gain: 10f32.powf(volume_db / 20.0),
            fade_in: fade_in.max(0),
            fade_out: fade_out.max(0),
            length,
        }
    }

    /// The gain `at` into the clip: the volume, times each fade's curve where one is under
    /// way; silence outside the clip.
    pub fn gain_at(&self, at: i64) -> f32 {
        if at < 0 || at >= self.length {
            return 0.0;
        }
        let mut gain = self.gain;
        if at < self.fade_in {
            gain *= curve(at, self.fade_in);
        }
        let left = self.length - at;
        if left < self.fade_out {
            gain *= curve(left, self.fade_out);
        }
        gain
    }

    /// Whether the clip plays as recorded throughout, so its samples need no scaling.
    pub fn is_unity(&self) -> bool {
        self.gain == 1.0 && self.fade_in == 0 && self.fade_out == 0
    }
}

/// A fade's gain `done` of the way through one `of` long: a quarter sine, so the power
/// rises evenly and the middle is 3 dB down.
fn curve(done: i64, of: i64) -> f32 {
    let fraction = done as f64 / of as f64;
    (fraction * std::f64::consts::FRAC_PI_2).sin() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = 1_000_000;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn no_volume_change_and_no_fades_leave_the_sound_alone() {
        let envelope = Envelope::new(0.0, 0, 0, 4 * SECOND);
        assert!(envelope.is_unity());
        for at in [0, SECOND, 4 * SECOND - 1] {
            assert_eq!(envelope.gain_at(at), 1.0);
        }
    }

    #[test]
    fn the_volume_is_in_decibels() {
        assert!(close(
            Envelope::new(-6.0, 0, 0, SECOND).gain_at(0),
            0.501_187
        ));
        assert!(close(
            Envelope::new(12.0, 0, 0, SECOND).gain_at(0),
            3.981_072
        ));
        assert!(close(Envelope::new(-60.0, 0, 0, SECOND).gain_at(0), 0.001));
        assert!(!Envelope::new(-6.0, 0, 0, SECOND).is_unity());
    }

    #[test]
    fn a_fade_in_rises_from_silence_with_constant_power() {
        let envelope = Envelope::new(0.0, SECOND, 0, 4 * SECOND);
        assert!(!envelope.is_unity());
        assert_eq!(envelope.gain_at(0), 0.0);
        // Halfway the gain is sin(45°): half the power, -3 dB.
        assert!(close(
            envelope.gain_at(SECOND / 2),
            std::f32::consts::FRAC_1_SQRT_2
        ));
        assert_eq!(envelope.gain_at(SECOND), 1.0);
        assert_eq!(envelope.gain_at(3 * SECOND), 1.0);
    }

    #[test]
    fn a_fade_out_falls_to_silence_at_the_end() {
        let envelope = Envelope::new(-6.0, 0, SECOND, 4 * SECOND);
        assert!(close(envelope.gain_at(3 * SECOND), 0.501_187));
        assert!(close(
            envelope.gain_at(3 * SECOND + SECOND / 2),
            0.501_187 * std::f32::consts::FRAC_1_SQRT_2
        ));
        assert_eq!(envelope.gain_at(4 * SECOND), 0.0);
        // Outside the clip it is silent.
        assert_eq!(envelope.gain_at(5 * SECOND), 0.0);
        assert_eq!(Envelope::new(0.0, 0, 0, SECOND).gain_at(-1), 0.0);
    }

    #[test]
    fn fades_that_meet_multiply() {
        let envelope = Envelope::new(0.0, 2 * SECOND, 2 * SECOND, 4 * SECOND);
        let middle = envelope.gain_at(2 * SECOND);
        assert!(close(middle, 1.0));
        let quarter = envelope.gain_at(SECOND);
        assert!(close(quarter, std::f32::consts::FRAC_1_SQRT_2));
    }
}
