//! The playback clock (docs/ARCHITECTURE.md, "Playback"): audio is the clock and video follows
//! it. While sound plays, the clock counts the frames the audio device has played; when it
//! cannot (no output device, a muted speed, playing backwards) the system clock stands in.

use std::sync::Arc;
use std::time::Instant;

/// Reports how many frames the audio device has played so far.
pub trait FrameCounter: Send + Sync {
    /// Frames played since the stream started, as close to what is being heard now as the
    /// device lets us know.
    fn frames_heard(&self) -> u64;
    /// Frames per second.
    fn rate(&self) -> u32;
}

/// What drives the clock while it runs.
#[derive(Clone)]
pub enum ClockSource {
    /// The audio device: time advances as it plays frames.
    Audio(Arc<dyn FrameCounter>),
    /// The system's monotonic clock.
    System,
}

/// Where playback is on the timeline, in microseconds of timeline time.
pub struct PlaybackClock {
    state: State,
}

enum State {
    Stopped {
        at: i64,
    },
    Running {
        start: i64,
        speed: f64,
        source: ClockSource,
        frames_at_start: u64,
        instant_at_start: Instant,
    },
}

impl PlaybackClock {
    /// A stopped clock at `at` microseconds.
    pub fn stopped(at: i64) -> PlaybackClock {
        PlaybackClock {
            state: State::Stopped { at },
        }
    }

    /// Starts running from `at` microseconds at `speed` (negative plays backwards), driven by
    /// `source`; `now` is the system time it starts at.
    pub fn start(&mut self, at: i64, speed: f64, source: ClockSource, now: Instant) {
        let frames_at_start = match &source {
            ClockSource::Audio(device) => device.frames_heard(),
            ClockSource::System => 0,
        };
        self.state = State::Running {
            start: at,
            speed,
            source,
            frames_at_start,
            instant_at_start: now,
        };
    }

    /// Stops where the clock is at `now` and returns that time.
    pub fn stop(&mut self, now: Instant) -> i64 {
        let at = self.time(now);
        self.state = State::Stopped { at };
        at
    }

    /// The timeline time at `now`, in microseconds.
    pub fn time(&self, now: Instant) -> i64 {
        match &self.state {
            State::Stopped { at } => *at,
            State::Running {
                start,
                speed,
                source,
                frames_at_start,
                instant_at_start,
            } => {
                let elapsed = match source {
                    ClockSource::Audio(device) => {
                        let frames = device.frames_heard().saturating_sub(*frames_at_start);
                        frames as f64 / f64::from(device.rate().max(1))
                    }
                    ClockSource::System => now
                        .saturating_duration_since(*instant_at_start)
                        .as_secs_f64(),
                };
                start + (elapsed * 1e6 * speed).round() as i64
            }
        }
    }

    /// Whether the clock is running.
    pub fn is_running(&self) -> bool {
        matches!(self.state, State::Running { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    struct FakeDevice(AtomicU64);

    impl FrameCounter for FakeDevice {
        fn frames_heard(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
        fn rate(&self) -> u32 {
            48_000
        }
    }

    #[test]
    fn a_stopped_clock_stays_put() {
        let clock = PlaybackClock::stopped(1_500_000);
        let now = Instant::now();
        assert_eq!(clock.time(now), 1_500_000);
        assert_eq!(clock.time(now + Duration::from_secs(5)), 1_500_000);
        assert!(!clock.is_running());
    }

    #[test]
    fn the_system_clock_runs_at_the_playback_speed() {
        let t0 = Instant::now();
        let mut clock = PlaybackClock::stopped(0);
        clock.start(1_000_000, 2.0, ClockSource::System, t0);
        assert!(clock.is_running());
        assert_eq!(clock.time(t0 + Duration::from_millis(250)), 1_500_000);
        assert_eq!(clock.stop(t0 + Duration::from_millis(500)), 2_000_000);
        assert_eq!(clock.time(t0 + Duration::from_secs(9)), 2_000_000);
    }

    #[test]
    fn backwards_playback_runs_the_clock_back() {
        let t0 = Instant::now();
        let mut clock = PlaybackClock::stopped(0);
        clock.start(1_000_000, -1.0, ClockSource::System, t0);
        assert_eq!(clock.time(t0 + Duration::from_millis(400)), 600_000);
    }

    #[test]
    fn the_audio_clock_follows_the_frames_played_not_the_wall_clock() {
        let device = Arc::new(FakeDevice(AtomicU64::new(96_000)));
        let t0 = Instant::now();
        let mut clock = PlaybackClock::stopped(0);
        clock.start(5_000_000, 1.0, ClockSource::Audio(device.clone()), t0);
        // Nothing played yet: still at the start, however much wall time passes.
        assert_eq!(clock.time(t0 + Duration::from_secs(1)), 5_000_000);
        device.0.store(96_000 + 24_000, Ordering::Relaxed);
        assert_eq!(clock.time(t0), 5_500_000);
        // At 2x every played second covers two seconds of timeline.
        let mut fast = PlaybackClock::stopped(0);
        fast.start(0, 2.0, ClockSource::Audio(device.clone()), t0);
        device.0.store(96_000 + 24_000 + 48_000, Ordering::Relaxed);
        assert_eq!(fast.time(t0), 2_000_000);
    }
}
