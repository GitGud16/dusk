//! Decodes the start of a clip with software and then hardware decoding, and prints how fast
//! each went and how much private memory the decoder held, for the decoder rules in
//! docs/ARCHITECTURE.md:
//!
//! ```text
//! cargo run -p dusk-media --release --example decode_speed -- clip.mp4
//! ```
//!
//! Frames are copied out as the app does (`VideoDecoder::frame_at`). Windows only.

#[cfg(windows)]
fn main() {
    use std::path::PathBuf;
    use std::time::Instant;

    use dusk_core::MediaTime;
    use dusk_media::{Acceleration, StreamDetail, VideoDecoder, probe};

    let path = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("usage: decode_speed <clip>"),
    );
    let info = probe(&path).expect("the clip probes");
    let (numerator, denominator) = info
        .streams
        .iter()
        .find_map(|stream| match stream.detail {
            StreamDetail::Video { frame_rate, .. } => frame_rate,
            _ => None,
        })
        .expect("a video stream with a frame rate");
    let frame_us = 1e6 * f64::from(denominator) / f64::from(numerator);
    let duration = info.duration_us.unwrap_or(10_000_000) as f64;
    let count = ((duration / frame_us) as i64).min(300);

    for acceleration in [Acceleration::Software, Acceleration::Hardware] {
        let before = private_mb();
        let start = Instant::now();
        let mut decoder = VideoDecoder::open(&path, acceleration).expect("the clip opens");
        let mut peak = before;
        for k in 0..count {
            let time = MediaTime((k as f64 * frame_us).round() as i64);
            decoder.frame_at(time).expect("decoding works");
            if k % 30 == 0 {
                peak = peak.max(private_mb());
            }
        }
        let fps = count as f64 / start.elapsed().as_secs_f64();
        let kind = if decoder.is_hardware() {
            "hardware"
        } else {
            "software"
        };
        drop(decoder);
        println!(
            "{kind:<9} {fps:6.0} fps   peak {:+6.0} MB   after close {:+6.0} MB",
            peak - before,
            private_mb() - before
        );
    }
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
    eprintln!("decode_speed reads Windows process counters; it does not run on this platform");
}
