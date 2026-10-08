//! Preview statistics for measuring (docs/ROADMAP.md, M1): with the `DUSK_STATS` environment
//! variable set, Dusk prints once a second how many frames the engine handed to the preview,
//! how many times the window was drawn, and how long the UI thread spent turning frames into
//! images. This is how the 60 fps preview check is run on other machines.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, RenderingState, Timer, TimerMode};

use crate::MainWindow;

/// Counters for the last second.
#[derive(Default)]
struct Counts {
    frames: Cell<u32>,
    drawn: Cell<u32>,
    busy: Cell<Duration>,
}

/// The statistics, when `DUSK_STATS` is set.
pub struct Stats {
    counts: Rc<Counts>,
    _timer: Timer,
}

impl Stats {
    /// Starts counting for `window` if `DUSK_STATS` is set.
    pub fn start(window: &MainWindow) -> Option<Stats> {
        std::env::var_os("DUSK_STATS")?;
        let counts = Rc::new(Counts::default());
        let drawn = Rc::clone(&counts);
        // Without a notifier (another renderer) only the frame counts are printed.
        let _ = window.window().set_rendering_notifier(move |state, _| {
            if let RenderingState::AfterRendering = state {
                drawn.drawn.set(drawn.drawn.get() + 1);
            }
        });
        let printed = Rc::clone(&counts);
        let timer = Timer::default();
        timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
            let frames = printed.frames.replace(0);
            let drawn = printed.drawn.replace(0);
            let busy = printed.busy.replace(Duration::ZERO);
            let per_frame = busy.as_secs_f64() * 1000.0 / f64::from(frames.max(1));
            eprintln!(
                "preview: {frames} frames received, {drawn} draws, {per_frame:.2} ms UI time a frame"
            );
        });
        Some(Stats {
            counts,
            _timer: timer,
        })
    }

    /// Counts a frame handed to the preview, which took the UI thread since `started`.
    pub fn frame(&self, started: Instant) {
        let counts = &self.counts;
        counts.frames.set(counts.frames.get() + 1);
        counts.busy.set(counts.busy.get() + started.elapsed());
    }
}
