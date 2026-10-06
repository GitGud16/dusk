//! What dusq says while it works: how far a job is and about how long it has left.

use std::time::Duration;

use dusk_engine::Progress;

/// `progress` of a job running for `elapsed`, as one line: the share done, the source time
/// reached of its length, and an estimate of the time left once there is enough to go by.
pub fn line(progress: Progress, elapsed: Duration) -> String {
    let seconds = |micros: i64| u64::try_from(micros).unwrap_or(0) / 1_000_000;
    let (done, total) = (progress.done.0, progress.total.0);
    if total <= 0 {
        return clock(seconds(done));
    }
    let share = done.clamp(0, total) as f64 / total as f64;
    let mut line = format!(
        "{}%  {} of {}",
        (share * 100.0).floor(),
        clock(seconds(done)),
        clock(seconds(total))
    );
    // A guess needs a few seconds and a few percent to go by.
    if elapsed >= Duration::from_secs(3) && share >= 0.02 {
        let left = elapsed.as_secs_f64() * (1.0 - share) / share;
        line += &format!("  about {} left", clock(left.round() as u64));
    }
    line
}

/// A length of time as minutes and seconds, or hours, minutes and seconds.
pub fn clock(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// A file size in megabytes, or kilobytes for small files.
pub fn size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else {
        format!("{:.0} KB", bytes as f64 / 1e3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::MediaTime;

    fn at(done: i64, total: i64) -> Progress {
        Progress {
            done: MediaTime(done * 1_000_000),
            total: MediaTime(total * 1_000_000),
        }
    }

    #[test]
    fn clocks_read_as_minutes_or_hours() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(83), "1:23");
        assert_eq!(clock(3_723), "1:02:03");
    }

    #[test]
    fn sizes_read_in_megabytes() {
        assert_eq!(size(25_000_000), "25.0 MB");
        assert_eq!(size(1_234_567), "1.2 MB");
        assert_eq!(size(56_789), "57 KB");
    }

    #[test]
    fn the_line_says_how_far_and_how_long_is_left() {
        assert_eq!(
            line(at(50, 200), Duration::from_secs(10)),
            "25%  0:50 of 3:20  about 0:30 left"
        );
        // Too early to tell.
        assert_eq!(line(at(1, 200), Duration::from_secs(1)), "0%  0:01 of 3:20");
        // A length the source does not state.
        assert_eq!(line(at(42, 0), Duration::from_secs(5)), "0:42");
    }
}
