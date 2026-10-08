//! Measures playback for the M1 checks (docs/ROADMAP.md): private bytes with a clip open,
//! during playback and 10 s after it stops, frames shown and skipped, and how long it takes
//! to show an exact frame after jumping to it:
//!
//! ```text
//! cargo run -p dusk-engine --release --example playback_probe -- clip.mp4 [seconds] [cache MiB]
//! ```
//!
//! `PROBE_SILENT` plays without opening the audio device; `PROBE_LONGER` also measures 30 s
//! after playback.
//!
//! It plays with sound, through the same engine as the app, into a 1280x720 preview. Windows
//! only, like the counters it reads.

#[cfg(windows)]
fn main() {
    use std::path::PathBuf;
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant};

    use dusk_core::{Frame, Project, import};
    use dusk_engine::{Engine, EngineEvent, EngineOptions, Gpu, media_info};

    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().expect("usage: playback_probe <clip> [seconds]"));
    let seconds: u64 = args
        .next()
        .and_then(|arg| arg.to_str()?.parse().ok())
        .unwrap_or(300);
    let mut options = EngineOptions {
        // PROBE_SILENT plays on the system clock without opening the audio device.
        sound: std::env::var_os("PROBE_SILENT").is_none(),
        ..EngineOptions::default()
    };
    if let Some(cap) = args
        .next()
        .and_then(|arg| arg.to_str()?.parse::<usize>().ok())
    {
        options.cache_cap = cap << 20;
    }

    let gpu = Gpu::new().expect("a graphics adapter");
    let (sender, events) = mpsc::channel();
    let engine = Engine::new(&gpu, options, move |event| {
        let _ = sender.send((Instant::now(), event));
    })
    .expect("the engine starts");
    let info = media_info(&path).expect("the clip probes");
    let rate = info.frame_rate.expect("a video clip");
    let mut project = Project::new(rate, (info.width, info.height));
    import(&project, path, info, Frame(0))
        .apply(&mut project)
        .expect("the clip imports");
    let end = project.sequence().end();
    engine.set_project(Arc::new(project));
    engine.set_preview_size((1280, 720));

    // Waits for the frame event of `frame`, reporting errors on the way.
    let wait_for = |frame: Frame| loop {
        match events.recv_timeout(Duration::from_secs(30)) {
            Ok((at, EngineEvent::Frame { frame: shown, .. })) if shown == frame => return at,
            Ok((_, EngineEvent::Error(error))) => eprintln!("error: {error}"),
            Ok(_) => {}
            Err(_) => panic!("frame {frame:?} never came"),
        }
    };

    engine.show(Frame(0));
    wait_for(Frame(0));
    // Decoders close after 5 s unused: what stays is the idle cost of an open project.
    std::thread::sleep(Duration::from_secs(6));
    println!("idle, clip open:          {:7.1} MB", private_mb());

    let played = Duration::from_secs(seconds).min(Duration::from_secs_f64(
        end.0 as f64 * f64::from(rate.den()) / f64::from(rate.num()) - 1.0,
    ));
    let started = Instant::now();
    engine.play(Frame(0), 1.0);
    let mut shown: Vec<(Instant, Frame)> = Vec::new();
    let mut peak = 0.0f64;
    let mut next_sample = started + Duration::from_secs(30);
    while started.elapsed() < played {
        match events.recv_timeout(Duration::from_millis(100)) {
            Ok((at, EngineEvent::Frame { frame, .. })) => shown.push((at, frame)),
            Ok((_, EngineEvent::Error(error))) => eprintln!("error: {error}"),
            Ok((_, EngineEvent::Stopped { frame })) => {
                println!("stopped by itself at {frame:?}");
                break;
            }
            _ => {}
        }
        if Instant::now() >= next_sample {
            let now = private_mb();
            peak = peak.max(now);
            println!(
                "playing {:4} s:           {now:7.1} MB, {} frames shown",
                started.elapsed().as_secs(),
                shown.len()
            );
            next_sample += Duration::from_secs(30);
        }
    }
    let stopped_at = engine.pause();
    let during = private_mb();
    peak = peak.max(during);
    println!("at the end of playback:   {during:7.1} MB (peak sampled {peak:.1} MB)");
    std::thread::sleep(Duration::from_secs(10));
    println!("10 s after playback:      {:7.1} MB", private_mb());
    if std::env::var_os("PROBE_LONGER").is_some() {
        std::thread::sleep(Duration::from_secs(20));
        println!("30 s after playback:      {:7.1} MB", private_mb());
    }

    // Smoothness: frames skipped, and the longest wait between two shown frames.
    let skipped: i64 = shown
        .windows(2)
        .map(|pair| (pair[1].1 - pair[0].1).0 - 1)
        .filter(|gap| *gap > 0)
        .sum();
    let mut intervals: Vec<f64> = shown
        .windows(2)
        .map(|pair| (pair[1].0 - pair[0].0).as_secs_f64() * 1000.0)
        .collect();
    intervals.sort_by(f64::total_cmp);
    let percentile = |p: f64| intervals[((intervals.len() - 1) as f64 * p) as usize];
    println!(
        "played {:.1} s to {stopped_at:?}: {} frames shown, {skipped} skipped; frame interval \
         median {:.1} ms, 99th percentile {:.1} ms, longest {:.1} ms",
        played.as_secs_f64(),
        shown.len(),
        percentile(0.5),
        percentile(0.99),
        percentile(1.0)
    );

    // Jumping: an exact frame far from anything cached, as after a click on the ruler.
    let mut latencies = Vec::new();
    let mut state = 0x2545_f491_u64;
    for _ in 0..30 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let frame = Frame((state >> 33) as i64 % end.0.max(1));
        let asked = Instant::now();
        engine.show(frame);
        latencies.push((wait_for(frame) - asked).as_secs_f64() * 1000.0);
    }
    latencies.sort_by(f64::total_cmp);
    println!(
        "jump to an exact frame: median {:.1} ms, longest {:.1} ms (30 jumps)",
        latencies[latencies.len() / 2],
        latencies[latencies.len() - 1]
    );

    // Dragging the playhead: one request every 16 ms over two seconds of footage.
    let drag_start = Instant::now();
    let mut answered = 0;
    let first = Frame(end.0 / 2);
    for step in 0..60 {
        engine.scrub(first + Frame(step));
        std::thread::sleep(Duration::from_millis(16));
        while let Ok((_, event)) = events.try_recv() {
            if matches!(event, EngineEvent::Frame { .. }) {
                answered += 1;
            }
        }
    }
    let last = first + Frame(59);
    engine.show(last);
    let settled = wait_for(last);
    println!(
        "drag over 60 frames in {:.0} ms: {answered} frames drawn on the way, the last one \
         exact {:.1} ms after the drag ended",
        (settled - drag_start).as_secs_f64() * 1000.0,
        (settled - drag_start).as_secs_f64() * 1000.0 - 60.0 * 16.0
    );
}

/// The process's private commit in megabytes (2^20 bytes, as Task Manager shows it).
#[cfg(windows)]
fn private_mb() -> f64 {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        size: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged_pool: usize,
        quota_paged_pool: usize,
        quota_peak_non_paged_pool: usize,
        quota_non_paged_pool: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
    }
    let mut counters = Counters {
        size: size_of::<Counters>() as u32,
        ..Default::default()
    };
    // SAFETY: the counters struct matches PROCESS_MEMORY_COUNTERS_EX and outlives the call.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.size) };
    counters.private as f64 / f64::from(1 << 20)
}

#[cfg(not(windows))]
fn main() {
    eprintln!("playback_probe reads Windows process counters; it does not run on this platform");
}
