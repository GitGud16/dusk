//! Exports encoded in Dusk's helper (docs/ARCHITECTURE.md, "Export"). The helper here is this
//! test program itself: started with [`SERVE`] among its arguments, its first test serves the
//! export as `dusq encode-for-dusk` does, so the tests run the real process, pipes and all,
//! without needing `dusq` built. Needs a graphics adapter; CI runners use WARP and OpenH264.
#![cfg(feature = "gpu")]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use dusk_core::{Frame, MediaTime, Project, import};
use dusk_engine::{
    CompressTarget, Engine, EngineError, EngineEvent, EngineOptions, ExportEvent, ExportSettings,
    Gpu, Helper, available_encoders, media_info, probe_encoders, serve_helper,
};
use dusk_media::{Acceleration, StreamKind, VideoDecoder, probe};

/// The argument that has this program serve as Dusk's helper.
const SERVE: &str = "dusk-test-serve-as-the-helper";

/// The argument that has it end at once, saying nothing, as a helper that crashed would.
const VANISH: &str = "dusk-test-vanish";

#[test]
fn serve_as_the_helper() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == VANISH) {
        std::process::exit(3);
    }
    if !args.iter().any(|arg| arg == SERVE) {
        return;
    }
    let served = serve_helper(std::io::stdin().lock(), std::io::stdout().lock());
    std::process::exit(if served { 0 } else { 1 });
}

/// This program, started to serve as Dusk's helper; `marker` says how.
fn this_program_as(marker: &str) -> Helper {
    // The test's own name and the marker are both filters, so only that test runs.
    let args = ["serve_as_the_helper", marker, "--exact", "--nocapture"];
    Helper {
        program: std::env::current_exe().expect("this test program"),
        args: args.into_iter().map(OsString::from).collect(),
    }
}

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-h264-aac.mp4")
}

const PATIENCE: Duration = Duration::from_secs(120);

/// A fresh output path in the target directory.
fn output(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("helper");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(part(&path));
    path
}

fn part(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

fn project() -> Arc<Project> {
    let info = media_info(&sample()).unwrap();
    let mut project = Project::new(info.frame_rate.unwrap(), (320, 240));
    import(&project, sample(), info, Frame(0))
        .apply(&mut project)
        .unwrap();
    Arc::new(project)
}

fn engine(helper: Option<Helper>) -> (Engine, mpsc::Receiver<EngineEvent>) {
    let gpu = Gpu::new().expect("a graphics adapter: hardware, or WARP on CI");
    let (sender, events) = mpsc::channel();
    let options = EngineOptions {
        sound: false,
        helper,
        ..EngineOptions::default()
    };
    let engine = Engine::new(&gpu, options, move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    (engine, events)
}

/// The export's outcome, and the progress reported before it.
fn outcome(events: &mpsc::Receiver<EngineEvent>) -> (ExportEvent, Vec<(u64, u64)>) {
    let mut progress = Vec::new();
    loop {
        match events.recv_timeout(PATIENCE).expect("the export ends") {
            EngineEvent::Export(ExportEvent::Progress { done, total }) => {
                progress.push((done, total));
            }
            EngineEvent::Export(outcome) => return (outcome, progress),
            _ => {}
        }
    }
}

/// Exports the project to `name` through `helper`, or in this process without one; the
/// encoder that wrote it.
fn export(helper: Option<Helper>, name: &str) -> (PathBuf, String) {
    let (engine, events) = engine(helper);
    let path = output(name);
    let _job = engine
        .export(project(), path.clone(), ExportSettings::default())
        .unwrap();
    let (outcome, progress) = outcome(&events);
    let encoder = match outcome {
        ExportEvent::Finished {
            path: written,
            encoder,
            ..
        } => {
            assert_eq!(written, path);
            encoder
        }
        other => panic!("expected the export to finish, got {other:?}"),
    };
    assert!(!part(&path).exists());
    assert_eq!(progress.last(), Some(&(30, 30)));
    (path, encoder)
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let squared: f64 = a
        .iter()
        .zip(b)
        .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
        .sum();
    10.0 * (255.0f64.powi(2) / (squared / a.len() as f64).max(1e-9)).log10()
}

#[test]
fn an_export_in_the_helper_comes_out_as_one_made_in_dusk() {
    let (helped, helper_encoder) = export(Some(this_program_as(SERVE)), "helped.mp4");
    let (here, own_encoder) = export(None, "here.mp4");
    // The same encoder of the order opened in both processes.
    assert_eq!(helper_encoder, own_encoder);
    let (helped_info, here_info) = (probe(&helped).unwrap(), probe(&here).unwrap());
    let kinds = |info: &dusk_media::ProbeInfo| -> Vec<StreamKind> {
        info.streams.iter().map(|stream| stream.kind).collect()
    };
    assert_eq!(kinds(&helped_info), kinds(&here_info));
    assert!(kinds(&helped_info).contains(&StreamKind::Audio));
    assert_eq!(helped_info.duration_us, here_info.duration_us);
    for time in [0, 500_000, 966_666] {
        let picture = |path: &Path| {
            let mut decoder = VideoDecoder::open(path, Acceleration::Software).unwrap();
            decoder.frame_at(MediaTime(time)).unwrap().unwrap().picture
        };
        let (a, b) = (picture(&helped), picture(&here));
        assert_eq!((a.width, a.height), (320, 240));
        let luma = psnr(&a.luma, &b.luma);
        assert!(luma >= 45.0, "{luma:.1} dB at {time} µs");
    }
}

#[test]
fn a_cancelled_export_in_the_helper_leaves_nothing_behind() {
    let (engine, events) = engine(Some(this_program_as(SERVE)));
    let path = output("cancelled.mp4");
    let job = engine
        .export(project(), path.clone(), ExportSettings::default())
        .unwrap();
    job.cancel();
    assert!(matches!(outcome(&events).0, ExportEvent::Cancelled));
    assert!(!path.exists());
    assert!(!part(&path).exists());
}

#[test]
fn a_helper_that_cannot_start_is_reported() {
    let missing = Helper::dusq(PathBuf::from("no such folder").join("dusq.exe"));
    let (engine, events) = engine(Some(missing));
    let path = output("never.mp4");
    let _job = engine
        .export(project(), path.clone(), ExportSettings::default())
        .unwrap();
    match outcome(&events).0 {
        ExportEvent::Failed(error @ EngineError::HelperStart { .. }) => {
            assert!(error.to_string().contains("Reinstall Dusk"), "{error}");
        }
        other => panic!("expected the export to fail, got {other:?}"),
    }
    assert!(!part(&path).exists());
}

#[test]
fn a_helper_that_ends_without_a_word_is_reported() {
    let (engine, events) = engine(Some(this_program_as(VANISH)));
    let path = output("vanished.mp4");
    let _job = engine
        .export(project(), path.clone(), ExportSettings::default())
        .unwrap();
    match outcome(&events).0 {
        ExportEvent::Failed(EngineError::HelperFailed { message }) => {
            assert!(message.contains("stopped"), "{message}");
        }
        other => panic!("expected the export to fail, got {other:?}"),
    }
    assert!(!part(&path).exists());
}

#[test]
fn the_helper_finds_the_encoders_that_open() {
    let names = |encoders: Vec<&dusk_engine::Encoder>| -> Vec<&'static str> {
        encoders.iter().map(|encoder| encoder.name).collect()
    };
    let helper = this_program_as(SERVE);
    assert_eq!(
        names(probe_encoders(Some(&helper))),
        names(available_encoders().to_vec())
    );
    // A helper that cannot say leaves the probe to this process.
    let vanished = this_program_as(VANISH);
    assert_eq!(
        names(probe_encoders(Some(&vanished))),
        names(available_encoders().to_vec())
    );
}

#[test]
fn the_compress_tool_makes_its_file_in_the_helper() {
    let (engine, events) = engine(Some(this_program_as(SERVE)));
    let path = output("compressed.mp4");
    let info = media_info(&sample()).unwrap();
    let _job = engine
        .compress(sample(), info, path.clone(), CompressTarget::Size(100_000))
        .unwrap();
    let outcome = loop {
        match events.recv_timeout(PATIENCE).expect("the compression ends") {
            EngineEvent::Export(ExportEvent::Progress { .. } | ExportEvent::Again { .. }) => {}
            EngineEvent::Export(outcome) => break outcome,
            _ => {}
        }
    };
    match outcome {
        ExportEvent::Finished { bytes, .. } => {
            assert!((40_000..=150_000).contains(&bytes), "{bytes} bytes");
        }
        other => panic!("expected the compression to finish, got {other:?}"),
    }
    assert!(!part(&path).exists());
}
