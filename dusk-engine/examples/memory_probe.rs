//! Prints how much private memory each part of the preview path adds, step by step, for the
//! memory budgets in docs/REQUIREMENTS.md:
//!
//! ```text
//! cargo run -p dusk-engine --release --example memory_probe -- clip.mp4 [--hardware]
//! ```
//!
//! "Private bytes" is the process's private commit, the figure the budgets use. Windows only.

#[cfg(windows)]
fn main() {
    use std::path::PathBuf;

    use dusk_core::MediaTime;
    use dusk_media::{Acceleration, VideoDecoder, probe};
    use dusk_render::{Compositor, Gpu};

    let path = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("usage: memory_probe <clip> [--hardware]"),
    );
    let acceleration = if std::env::args().any(|arg| arg == "--hardware") {
        Acceleration::Hardware
    } else {
        Acceleration::Software
    };
    let mut last = private_mb();
    let mut step = |name: &str| {
        let now = private_mb();
        println!("{name:<44} {now:7.1} MB  {:+7.1}", now - last);
        last = now;
    };
    step("start");
    probe(&path).expect("the clip probes");
    step("FFmpeg loaded, clip probed");
    let gpu = Gpu::new().expect("a graphics adapter");
    step(&format!(
        "GPU device ({:?})",
        gpu.adapter.get_info().backend
    ));
    let compositor = Compositor::new(&gpu);
    step("compositor pipelines");
    // A tiny frame first: what remains is pipeline compilation and allocator setup, not the
    // size of the textures.
    let tiny = dusk_core::Picture {
        width: 16,
        height: 16,
        layout: dusk_core::PictureLayout::Nv12,
        matrix: dusk_core::ColorMatrix::Bt709,
        range: dusk_core::ColorRange::Limited,
        primaries: dusk_core::color::Primaries::Bt709,
        transfer: dusk_core::color::Transfer::Bt1886,
        peak_nits: 0,
        luma: vec![128; 16 * 16],
        chroma: vec![128; 8 * 8 * 2],
    };
    let texture = compositor.render(&tiny, (16, 16)).unwrap();
    compositor.read_rgba(&texture).unwrap();
    step("a 16x16 frame drawn");
    let mut decoder = VideoDecoder::open(&path, acceleration).expect("the clip opens");
    step("decoder opened");
    let frame = decoder
        .frame_at(MediaTime(0))
        .unwrap()
        .expect("a first frame");
    let kind = if decoder.is_hardware() {
        "hardware"
    } else {
        "software"
    };
    step(&format!("first frame decoded ({kind})"));
    let texture = compositor.render(&frame.picture, (1280, 720)).unwrap();
    compositor.read_rgba(&texture).unwrap();
    step("first frame drawn at 1280x720");
    for n in 1..=30 {
        let time = MediaTime(n * 33_367);
        let frame = decoder.frame_at(time).unwrap().expect("a frame");
        let texture = compositor.render(&frame.picture, (1280, 720)).unwrap();
        compositor.read_rgba(&texture).unwrap();
    }
    step("30 more frames decoded and drawn");
    drop((frame, texture));
    step("last frame and texture dropped");
    drop(decoder);
    step("decoder closed");
    // Reopening shows whether closing gives memory back or leaks it. `--decode-only` and
    // `--render-only` separate the decoder's share from the compositor's.
    let decode = !std::env::args().any(|arg| arg == "--render-only");
    let render = !std::env::args().any(|arg| arg == "--decode-only");
    let picture = VideoDecoder::open(&path, Acceleration::Software)
        .expect("the clip opens")
        .frame_at(MediaTime(0))
        .unwrap()
        .expect("a frame")
        .picture;
    step("a software-decoded picture kept for --render-only");
    for round in 2..=6 {
        let mut decoder =
            decode.then(|| VideoDecoder::open(&path, acceleration).expect("the clip opens"));
        for n in 0..10 {
            let decoded = decoder.as_mut().map(|decoder| {
                decoder
                    .frame_at(MediaTime(n * 33_367))
                    .unwrap()
                    .expect("a frame")
            });
            if render {
                let source = decoded.as_ref().map_or(&picture, |frame| &frame.picture);
                let texture = compositor.render(source, (1280, 720)).unwrap();
                compositor.read_rgba(&texture).unwrap();
            }
        }
        drop(decoder);
        step(&format!("round {round}: decode {decode}, render {render}"));
    }
    drop(compositor);
    let _ = gpu
        .device
        .poll(dusk_render::wgpu::PollType::wait_indefinitely());
    step("compositor dropped, GPU idle");
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
    eprintln!("memory_probe reads Windows process counters; it does not run on this platform");
}
