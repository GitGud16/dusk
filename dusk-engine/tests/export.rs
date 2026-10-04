//! Exporting the sample (one second at 30 fps, 320x240, with a tone) to MP4. Needs a graphics
//! adapter; CI runners use WARP and OpenH264.
#![cfg(feature = "gpu")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use dusk_core::{Command, Edge, Frame, MediaTime, Project, TrimClips, import};
use dusk_engine::{Engine, EngineEvent, EngineOptions, ExportEvent, Gpu, media_info};
use dusk_media::{Acceleration, StreamKind, VideoDecoder, probe};

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-h264-aac.mp4")
}

const PATIENCE: Duration = Duration::from_secs(120);

/// A fresh output path in the target directory.
fn output(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("export");
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

fn project_at(position: Frame) -> Project {
    let info = media_info(&sample()).unwrap();
    let mut project = Project::new(info.frame_rate.unwrap(), (320, 240));
    import(&project, sample(), info, position)
        .apply(&mut project)
        .unwrap();
    project
}

fn engine() -> (Engine, mpsc::Receiver<EngineEvent>) {
    let gpu = Gpu::new().expect("a graphics adapter: hardware, or WARP on CI");
    let (sender, events) = mpsc::channel();
    let options = EngineOptions {
        sound: false,
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

fn duration_of(path: &Path) -> i64 {
    probe(path).unwrap().duration_us.unwrap()
}

#[test]
fn a_clip_exports_to_a_playable_mp4() {
    let (engine, events) = engine();
    let path = output("clip.mp4");
    let _job = engine
        .export(Arc::new(project_at(Frame(0))), path.clone())
        .unwrap();
    let (outcome, progress) = outcome(&events);
    match outcome {
        ExportEvent::Finished { path: written, .. } => assert_eq!(written, path),
        other => panic!("expected the export to finish, got {other:?}"),
    }
    assert!(path.is_file());
    assert!(!part(&path).exists());
    assert_eq!(progress.last(), Some(&(30, 30)));
    assert!(progress.windows(2).all(|pair| pair[0].0 < pair[1].0));

    let info = probe(&path).unwrap();
    let kinds: Vec<StreamKind> = info.streams.iter().map(|stream| stream.kind).collect();
    assert!(kinds.contains(&StreamKind::Video) && kinds.contains(&StreamKind::Audio));
    let duration = info.duration_us.unwrap();
    assert!((950_000..=1_100_000).contains(&duration), "{duration} µs");
    let mut decoder = VideoDecoder::open(&path, Acceleration::Software).unwrap();
    let frame = decoder.frame_at(MediaTime(500_000)).unwrap().unwrap();
    assert_eq!((frame.picture.width, frame.picture.height), (320, 240));
}

#[test]
fn a_trimmed_clip_exports_only_what_is_left() {
    let mut project = project_at(Frame(0));
    let clip = project.sequence().tracks()[0].clips()[0].id;
    Command::TrimClips(TrimClips::new(clip, Edge::End, Frame(15)))
        .apply(&mut project)
        .unwrap();
    let (engine, events) = engine();
    let path = output("trimmed.mp4");
    let _job = engine.export(Arc::new(project), path.clone()).unwrap();
    assert!(matches!(outcome(&events).0, ExportEvent::Finished { .. }));
    let duration = duration_of(&path);
    assert!((450_000..=600_000).contains(&duration), "{duration} µs");
}

#[test]
fn a_gap_exports_as_black() {
    let (engine, events) = engine();
    let path = output("gap.mp4");
    let _job = engine
        .export(Arc::new(project_at(Frame(15))), path.clone())
        .unwrap();
    assert!(matches!(outcome(&events).0, ExportEvent::Finished { .. }));
    let duration = duration_of(&path);
    assert!((1_450_000..=1_600_000).contains(&duration), "{duration} µs");
    let mut decoder = VideoDecoder::open(&path, Acceleration::Software).unwrap();
    let black = decoder.frame_at(MediaTime(200_000)).unwrap().unwrap();
    assert!(black.picture.luma.iter().all(|code| code.abs_diff(16) <= 2));
}

#[test]
fn a_cancelled_export_leaves_nothing_behind() {
    let (engine, events) = engine();
    let path = output("cancelled.mp4");
    let job = engine
        .export(Arc::new(project_at(Frame(0))), path.clone())
        .unwrap();
    job.cancel();
    assert!(matches!(outcome(&events).0, ExportEvent::Cancelled));
    assert!(!path.exists());
    assert!(!part(&path).exists());
}

#[test]
fn an_empty_timeline_is_refused() {
    let (engine, events) = engine();
    let info = media_info(&sample()).unwrap();
    let empty = Project::new(info.frame_rate.unwrap(), (320, 240));
    let path = output("empty.mp4");
    let _job = engine.export(Arc::new(empty), path.clone()).unwrap();
    assert!(matches!(outcome(&events).0, ExportEvent::Failed(_)));
    assert!(!path.exists());
    assert!(!part(&path).exists());
}

#[test]
fn closing_the_engine_mid_export_leaves_no_part_file() {
    let (engine, events) = engine();
    let path = output("closed.mp4");
    // Ten seconds of black before the clip: long enough to close the engine halfway.
    let _job = engine
        .export(Arc::new(project_at(Frame(300))), path.clone())
        .unwrap();
    loop {
        match events.recv_timeout(PATIENCE).expect("progress") {
            EngineEvent::Export(ExportEvent::Progress { .. }) => break,
            EngineEvent::Export(other) => panic!("the export ended early: {other:?}"),
            _ => {}
        }
    }
    assert!(part(&path).exists());
    drop(engine);
    assert!(!part(&path).exists());
    assert!(!path.exists());
}

#[test]
fn one_export_runs_at_a_time() {
    let (engine, events) = engine();
    let first = output("first.mp4");
    let _job = engine
        .export(Arc::new(project_at(Frame(0))), first)
        .unwrap();
    let second = output("second.mp4");
    assert!(
        engine
            .export(Arc::new(project_at(Frame(0))), second.clone())
            .is_err()
    );
    outcome(&events);
    assert!(!second.exists());
}
