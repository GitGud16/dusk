//! J, K and L (docs/FEATURES.md, "Preview"): playback from very slow to very fast, forwards
//! and backwards. L and J play at normal speed, then twice as fast with every press, up to
//! [`FASTEST`]; with Shift they play slowly, slower with every press, down to 0.1x.

/// The fastest playback, either way.
pub const FASTEST: f64 = 32.0;

/// The slow speeds Shift+L and Shift+J step through.
const SLOW: [f64; 3] = [0.5, 0.25, 0.1];

/// A speed key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedKey {
    /// L forwards or J backwards: normal speed, then faster.
    Faster { forward: bool },
    /// Shift+L forwards or Shift+J backwards: slow, then slower.
    Slower { forward: bool },
}

/// The playback factor `key` goes to from `current`, the factor while playing (negative
/// backwards) or `None` while stopped.
pub fn next_factor(current: Option<f64>, key: SpeedKey) -> f64 {
    let (forward, faster) = match key {
        SpeedKey::Faster { forward } => (forward, true),
        SpeedKey::Slower { forward } => (forward, false),
    };
    let sign = if forward { 1.0 } else { -1.0 };
    // How fast it plays now in the key's direction, if it plays that way at all.
    let speed = current
        .map(|factor| factor * sign)
        .filter(|speed| *speed > 0.0);
    let next = match (faster, speed) {
        (true, Some(speed)) if speed >= 1.0 => (speed * 2.0).min(FASTEST),
        (true, _) => 1.0,
        (false, Some(speed)) if speed < 1.0 => SLOW
            .into_iter()
            .find(|slow| *slow < speed)
            .unwrap_or(SLOW[SLOW.len() - 1]),
        (false, _) => SLOW[0],
    };
    sign * next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presses(key: SpeedKey, count: usize) -> Vec<f64> {
        let mut factor = None;
        (0..count)
            .map(|_| {
                let next = next_factor(factor, key);
                factor = Some(next);
                next
            })
            .collect()
    }

    #[test]
    fn l_and_j_double_up_to_the_fastest() {
        assert_eq!(
            presses(SpeedKey::Faster { forward: true }, 7),
            [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 32.0]
        );
        assert_eq!(
            presses(SpeedKey::Faster { forward: false }, 7),
            [-1.0, -2.0, -4.0, -8.0, -16.0, -32.0, -32.0]
        );
    }

    #[test]
    fn shift_slows_down_to_a_tenth() {
        assert_eq!(
            presses(SpeedKey::Slower { forward: true }, 4),
            [0.5, 0.25, 0.1, 0.1]
        );
        assert_eq!(
            presses(SpeedKey::Slower { forward: false }, 4),
            [-0.5, -0.25, -0.1, -0.1]
        );
    }

    #[test]
    fn a_key_for_the_other_way_starts_over() {
        let forward = SpeedKey::Faster { forward: true };
        let backward = SpeedKey::Faster { forward: false };
        assert_eq!(next_factor(Some(-4.0), forward), 1.0);
        assert_eq!(next_factor(Some(8.0), backward), -1.0);
        assert_eq!(
            next_factor(Some(-0.25), SpeedKey::Slower { forward: true }),
            0.5
        );
        // From slow, L goes back to normal speed; from fast, Shift+L goes to slow.
        assert_eq!(next_factor(Some(0.25), forward), 1.0);
        assert_eq!(
            next_factor(Some(4.0), SpeedKey::Slower { forward: true }),
            0.5
        );
    }
}
