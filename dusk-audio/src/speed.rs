//! Playing audio faster or slower than recorded: the pitch follows the speed, as on a tape
//! machine (docs/ARCHITECTURE.md, "Playback": no pitch correction in 0.1).

/// Resamples interleaved stereo by a playback speed, one block at a time, with linear
/// interpolation. At 2x every output frame advances two input frames, so a 440 Hz tone comes
/// out at 880 Hz.
pub struct SpeedResampler {
    speed: f64,
    /// The last input frame of the previous block, which interpolation into this block needs.
    previous: Option<[f32; 2]>,
    /// Where the next output frame sits among the input frames: index 0 is `previous` when
    /// there is one, else the first frame of the next block.
    position: f64,
}

impl SpeedResampler {
    /// A resampler for `speed` (1.0 is normal speed; it must be positive).
    pub fn new(speed: f64) -> SpeedResampler {
        SpeedResampler {
            speed,
            previous: None,
            position: 0.0,
        }
    }

    /// Resamples `input` (interleaved stereo) and appends the result to `output`. Blocks may
    /// be any size: the position between input frames carries over to the next call.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        let (block, _) = input.as_chunks::<2>();
        let offset = usize::from(self.previous.is_some());
        let available = block.len() + offset;
        let frame = |index: usize| match (index.checked_sub(offset), self.previous) {
            (Some(at), _) => block[at],
            (None, Some(previous)) => previous,
            (None, None) => [0.0; 2],
        };
        loop {
            let index = self.position as usize;
            let fraction = (self.position - index as f64) as f32;
            if index >= available {
                break;
            }
            if fraction == 0.0 {
                output.extend_from_slice(&frame(index));
            } else if index + 1 < available {
                let (a, b) = (frame(index), frame(index + 1));
                output.extend([0, 1].map(|c| a[c] + (b[c] - a[c]) * fraction));
            } else {
                break;
            }
            self.position += self.speed;
        }
        if available > 0 {
            self.previous = Some(frame(available - 1));
            self.position -= (available - 1) as f64;
        }
    }

    /// The playback speed.
    pub fn speed(&self) -> f64 {
        self.speed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second of a stereo sine at `hz`, sampled at 48 kHz.
    fn tone(hz: f64) -> Vec<f32> {
        (0..48_000)
            .flat_map(|n| {
                let sample = (2.0 * std::f64::consts::PI * hz * n as f64 / 48_000.0).sin() as f32;
                [sample, sample]
            })
            .collect()
    }

    fn crossings_per_second(stereo: &[f32]) -> f64 {
        let left: Vec<f32> = stereo.iter().step_by(2).copied().collect();
        let up = left
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        up as f64 * 48_000.0 / left.len() as f64
    }

    #[test]
    fn normal_speed_passes_samples_through() {
        let input = tone(440.0);
        let mut output = Vec::new();
        SpeedResampler::new(1.0).process(&input, &mut output);
        assert_eq!(output, input);
    }

    #[test]
    fn double_speed_halves_the_length_and_doubles_the_pitch() {
        let mut output = Vec::new();
        SpeedResampler::new(2.0).process(&tone(440.0), &mut output);
        assert!(
            (output.len() / 2).abs_diff(24_000) <= 1,
            "{} frames",
            output.len() / 2
        );
        let hz = crossings_per_second(&output);
        assert!((875.0..885.0).contains(&hz), "{hz} Hz");
    }

    #[test]
    fn half_speed_doubles_the_length() {
        let mut output = Vec::new();
        SpeedResampler::new(0.5).process(&tone(440.0), &mut output);
        assert!(
            (output.len() / 2).abs_diff(96_000) <= 2,
            "{} frames",
            output.len() / 2
        );
        let hz = crossings_per_second(&output);
        assert!((217.0..223.0).contains(&hz), "{hz} Hz");
    }

    #[test]
    fn blocks_join_seamlessly() {
        let input = tone(440.0);
        let mut whole = Vec::new();
        SpeedResampler::new(1.5).process(&input, &mut whole);
        let mut pieces = Vec::new();
        let mut resampler = SpeedResampler::new(1.5);
        for block in input.chunks(2 * 333) {
            resampler.process(block, &mut pieces);
        }
        assert_eq!(pieces.len(), whole.len());
        assert!(pieces.iter().zip(&whole).all(|(a, b)| (a - b).abs() < 1e-6));
    }
}
