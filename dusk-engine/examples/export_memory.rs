//! Measures what exports leave in Dusk's own process (docs/ROADMAP.md, M7): its private bytes
//! before, at most during and after each of a few exports of a clip, with the encoder in
//! Dusk's helper or in this process.
//!
//! ```text
//! cargo run -p dusk-engine --release --example export_memory -- clip.mp4 [h264|hevc|av1|vp9] [--helper target\release\dusq.exe]
//! ```
//!
//! The encoder probe runs first, where the exports run; then one small export (240p) warms up
//! what every first export sets up once in a process (the export pass's GPU pipelines, the
//! decoders' tables); then the clip is exported at its own size, as MP4 (MKV for VP9) at the
//! High preset, three times in a row. Windows only, like the counters it reads.

#[cfg(windows)]
fn main() {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use dusk_core::{Frame, Project, import};
    use dusk_engine::{
        AudioCodec, Container, Engine, EngineEvent, EngineOptions, ExportEvent, ExportFormat,
        ExportSettings, Gpu, Helper, VideoCodec, media_info, probe_encoders,
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let clip = PathBuf::from(
        args.first()
            .expect("usage: export_memory <clip> [codec] [--helper dusq]"),
    );
    let codec = match args.get(1).map(String::as_str) {
        Some("hevc") => VideoCodec::Hevc,
        Some("av1") => VideoCodec::Av1,
        Some("vp9") => VideoCodec::Vp9,
        _ => VideoCodec::H264,
    };
    let helper = args
        .iter()
        .position(|arg| arg == "--helper")
        .and_then(|at| args.get(at + 1))
        .map(|path| Helper::dusq(PathBuf::from(path)));
    let (container, audio) = if codec == VideoCodec::Vp9 {
        (Container::Mkv, AudioCodec::Opus)
    } else {
        (Container::Mp4, AudioCodec::Aac)
    };

    let gpu = Gpu::new().expect("a graphics adapter");
    let (sender, events) = mpsc::channel();
    let options = EngineOptions {
        sound: false,
        helper: helper.clone(),
        ..EngineOptions::default()
    };
    let engine = Engine::new(&gpu, options, move |event| {
        let _ = sender.send(event);
    })
    .expect("the engine starts");
    let info = media_info(&clip).expect("the clip can be read");
    let size = (info.width, info.height);
    let mut project = Project::new(info.frame_rate.expect("a video"), size);
    import(&project, clip.clone(), info, Frame(0))
        .apply(&mut project)
        .expect("the clip imports");
    let project = Arc::new(project);
    let settings = ExportSettings {
        format: ExportFormat::Video {
            container,
            codec,
            audio,
        },
        ..ExportSettings::default()
    };
    let settle = || std::thread::sleep(Duration::from_secs(3));
    println!(
        "{} at {}x{}, {} in {}",
        clip.display(),
        size.0,
        size.1,
        codec.name(),
        if helper.is_some() {
            "the helper"
        } else {
            "this process"
        }
    );
    settle();
    println!("start: {:.0} MB", private_mb());
    let found = probe_encoders(helper.as_ref());
    settle();
    println!(
        "after the probe ({} encoders): {:.0} MB",
        found.len(),
        private_mb()
    );
    let output = std::env::temp_dir().join(format!("dusk-export-memory.{}", container.extension()));
    let export = |settings: ExportSettings| {
        let _job = engine
            .export(Arc::clone(&project), output.clone(), settings)
            .expect("the export starts");
        loop {
            match events.recv().expect("the export ends") {
                EngineEvent::Export(ExportEvent::Finished { encoder, .. }) => return encoder,
                EngineEvent::Export(ExportEvent::Failed(error)) => panic!("{error}"),
                EngineEvent::Export(ExportEvent::Cancelled) => panic!("cancelled"),
                _ => {}
            }
        }
    };
    let before = private_mb();
    let encoder = export(ExportSettings {
        short_side: Some(240),
        ..settings
    });
    settle();
    println!(
        "warm-up at 240p through {encoder}: {before:.0} MB before, {:.0} MB 3 s after",
        private_mb()
    );
    for round in 1..=3 {
        let before = private_mb();
        // The most this process holds while the export runs.
        let running = Arc::new(AtomicBool::new(true));
        let peak = std::thread::spawn({
            let running = Arc::clone(&running);
            move || {
                let mut peak: f64 = 0.0;
                while running.load(Ordering::Relaxed) {
                    peak = peak.max(private_mb());
                    std::thread::sleep(Duration::from_millis(50));
                }
                peak
            }
        });
        let encoder = export(settings);
        running.store(false, Ordering::Relaxed);
        let peak = peak.join().unwrap_or(0.0);
        settle();
        println!(
            "export {round} through {encoder}: {before:.0} MB before, {peak:.0} MB at most, {:.0} MB 3 s after",
            private_mb()
        );
    }
    let _ = std::fs::remove_file(&output);
}

/// This process's private bytes, in MB.
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
    eprintln!("export_memory reads Windows process counters; it does not run on this platform");
}
