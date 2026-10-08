//! Prints how much private memory one encoder takes while an export drives it, for the
//! export budget's encoder line in docs/REQUIREMENTS.md: NV12 frames of the given size,
//! written as the export writes them, with moving detail so the encoder has work to do.
//!
//! ```text
//! cargo run -p dusk-media --release --example encoder_memory -- libsvtav1 1920x1080 [frames] [quality] [rounds]
//! ```
//!
//! Each run measures one encoder in a fresh process, so no other encoder's memory counts.
//! With more than one round, the encoder is opened again for each, which shows whether what
//! it keeps after closing grows with every export.
//! "Private bytes" is the process's private commit, the figure the budgets use, here in MB of
//! 10^6 bytes; the peak is the system's own record of it. Windows only.

#[cfg(windows)]
fn main() {
    use std::time::Instant;

    use dusk_core::color::{Primaries, Transfer};
    use dusk_core::{ChromaSiting, ColorMatrix, ColorRange, Picture, PictureLayout};
    use dusk_media::{Container, Quality, VideoSettings, Writer, encoder_named};

    let usage = "usage: encoder_memory <encoder> <width>x<height> [frames] [quality] [rounds]";
    let mut args = std::env::args().skip(1);
    let name = args.next().expect(usage);
    let encoder = encoder_named(&name).expect("an encoder of Dusk's table");
    let size = args.next().expect(usage);
    let (width, height) = size
        .split_once('x')
        .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)))
        .expect(usage);
    let frames: usize = args.next().map_or(300, |count| count.parse().expect(usage));
    let level: u8 = args.next().map_or(80, |level| level.parse().expect(usage));
    let rounds: usize = args.next().map_or(1, |rounds| rounds.parse().expect(usage));

    // Eight frames, cycled: bars that move, a slope, and noise the encoder cannot skip.
    let mut seed = 0x9e37_79b9_u32;
    let mut noise = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 27) as u8
    };
    let pictures: Vec<Picture> = (0..8u32)
        .map(|n| {
            let luma = (0..height)
                .flat_map(|y| (0..width).map(move |x| (x, y)))
                .map(|(x, y)| {
                    let bars = ((x + 24 * n) * 7 / width) as u8 * 24;
                    let slope = ((x + y) / 16 % 64) as u8;
                    40 + bars + slope
                })
                .zip(std::iter::repeat_with(&mut noise))
                .map(|(value, grain)| value.saturating_add(grain))
                .collect();
            let chroma = (0..height / 2)
                .flat_map(|y| (0..width).map(move |x| (x, y)))
                .map(|(x, y)| 96 + ((x / 2 + y + 8 * n) / 32 % 64) as u8)
                .collect();
            Picture {
                width,
                height,
                layout: PictureLayout::Nv12,
                matrix: ColorMatrix::Bt709,
                range: ColorRange::Limited,
                primaries: Primaries::Bt709,
                transfer: Transfer::Bt1886,
                peak_nits: 0,
                siting: ChromaSiting::LEFT,
                luma,
                chroma,
            }
        })
        .collect();

    let path = std::env::temp_dir().join(format!("dusk-encoder-memory-{}.mkv", std::process::id()));
    let (before, _) = private();
    let mb = |bytes: usize| bytes as f64 / 1e6;
    for round in 1..=rounds {
        let started = Instant::now();
        let video = VideoSettings {
            codec: encoder.codec,
            quality: Quality::Level(level),
            encoder: Some(encoder.name),
            ..VideoSettings::h264(width, height, (30, 1))
        };
        let mut writer = Writer::create(&path, Container::Mkv, video, None).expect("it opens");
        let (opened, _) = private();
        for n in 0..frames {
            writer
                .write_video(&pictures[n % pictures.len()])
                .expect("the frame is encoded");
        }
        let (writing, _) = private();
        writer.finish().expect("the file is finished");
        let seconds = started.elapsed().as_secs_f64();
        let (after, peak) = private();
        let bytes = std::fs::metadata(&path).map_or(0, |file| file.len());
        let _ = std::fs::remove_file(&path);
        println!(
            "{name} at {width}x{height}, quality {level}, round {round}: {:+.0} MB opened, \
             {:+.0} MB writing, peak {:+.0} MB, {:+.0} MB after; {frames} frames at {:.0} fps, \
             {:.1} MB written",
            mb(opened) - mb(before),
            mb(writing) - mb(before),
            mb(peak) - mb(before),
            mb(after) - mb(before),
            frames as f64 / seconds,
            bytes as f64 / 1e6,
        );
    }
}

/// The process's private commit and its peak so far, in bytes.
#[cfg(windows)]
fn private() -> (usize, usize) {
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
    // The commit charge and its peak are the same figure as private bytes and its peak.
    (counters.private, counters.peak_pagefile)
}

#[cfg(not(windows))]
fn main() {
    eprintln!("encoder_memory reads Windows process counters; it does not run on this platform");
}
