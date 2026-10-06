//! Exporting the sample (one second at 30 fps, 320x240, with a tone) to MP4. Needs a graphics
//! adapter; CI runners use WARP and OpenH264.
#![cfg(feature = "gpu")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use dusk_core::{
    ClipEditSession, Command, Edge, Frame, MediaTime, Project, Rational, Rect, Rotation,
    SetTrackMuted, TrimClips, import,
};
use dusk_engine::{
    Engine, EngineError, EngineEvent, EngineOptions, ExportEvent, ExportFormat, ExportSettings,
    Gpu, media_info,
};
use dusk_media::{Acceleration, StreamKind, VideoDecoder, probe};
use dusk_media::{AudioCodec, AudioFormat, Container, Quality, VideoCodec};

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
        .export(
            Arc::new(project_at(Frame(0))),
            path.clone(),
            ExportSettings::default(),
        )
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
fn a_clip_from_the_clip_editor_exports_at_its_own_rate_and_shape() {
    // A 25 fps 1280x720 sequence; the 30 fps sample is cropped and turned a quarter.
    let info = media_info(&sample()).unwrap();
    let mut project = Project::new(Rational::new(25, 1).unwrap(), (1280, 720));
    import(&project, sample(), info.clone(), Frame(0))
        .apply(&mut project)
        .unwrap();
    let clip = project.sequence().tracks()[0].clips()[0].id;
    let mut session = ClipEditSession::open(&project, clip).unwrap();
    let edits = session.draft.video.as_mut().unwrap();
    // An odd size, which the encoder rounds down to even.
    edits.crop = Some(Rect {
        x: 20,
        y: 10,
        width: 201,
        height: 121,
    });
    edits.rotate = Rotation::Quarter;
    let export = session.export_project(&project).unwrap();
    assert_eq!(export.sequence().resolution(), (121, 201));
    let (engine, events) = engine();
    let path = output("clip-editor.mp4");
    let _job = engine
        .export(Arc::new(export), path.clone(), ExportSettings::default())
        .unwrap();
    assert!(matches!(outcome(&events).0, ExportEvent::Finished { .. }));
    let written = media_info(&path).unwrap();
    assert_eq!((written.width, written.height), (120, 200));
    assert_eq!(written.frame_rate, info.frame_rate);
    assert!(written.has_audio);
    let duration = duration_of(&path);
    assert!((950_000..=1_100_000).contains(&duration), "{duration} µs");
    // The picture fills its frame: no bar where the size was rounded.
    let mut decoder = VideoDecoder::open(&path, Acceleration::Software).unwrap();
    let frame = decoder.frame_at(MediaTime(500_000)).unwrap().unwrap();
    let luma = &frame.picture.luma;
    let width = frame.picture.width as usize;
    let height = frame.picture.height as usize;
    let row_mean = |row: usize| {
        luma[row * width..(row + 1) * width]
            .iter()
            .map(|code| u32::from(*code))
            .sum::<u32>()
            / width as u32
    };
    let column_mean = |column: usize| {
        (0..height)
            .map(|row| u32::from(luma[row * width + column]))
            .sum::<u32>()
            / height as u32
    };
    let edges = [
        row_mean(0),
        row_mean(height - 1),
        column_mean(0),
        column_mean(width - 1),
    ];
    let inside = [
        row_mean(1),
        row_mean(height - 2),
        column_mean(1),
        column_mean(width - 2),
    ];
    for (edge, inside) in edges.iter().zip(inside) {
        assert!(
            edge.abs_diff(inside) <= 12,
            "edges {edges:?}, beside one of them {inside}"
        );
    }
}

/// Exports `project` with `settings` and waits for it to finish.
fn export_with(project: Project, name: &str, settings: ExportSettings) -> PathBuf {
    let (engine, events) = engine();
    let path = output(name);
    let _job = engine
        .export(Arc::new(project), path.clone(), settings)
        .unwrap();
    match outcome(&events).0 {
        ExportEvent::Finished { .. } => path,
        other => panic!("expected the export to finish, got {other:?}"),
    }
}

fn video_codec(path: &Path) -> String {
    probe(path)
        .unwrap()
        .streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Video)
        .map(|stream| stream.codec.clone())
        .unwrap_or_default()
}

fn audio_codec(path: &Path) -> String {
    probe(path)
        .unwrap()
        .streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Audio)
        .map(|stream| stream.codec.clone())
        .unwrap_or_default()
}

#[test]
fn a_preset_exports_the_short_side_keeping_the_shape() {
    // The sample is 320x240; a 120-pixel short side makes it 160x120.
    let settings = ExportSettings {
        short_side: Some(120),
        ..ExportSettings::default()
    };
    let path = export_with(project_at(Frame(0)), "short-side.mp4", settings);
    let written = media_info(&path).unwrap();
    assert_eq!((written.width, written.height), (160, 120));
}

#[test]
fn hevc_in_mkv_and_vp9_in_webm_export_from_the_timeline() {
    let mkv = ExportSettings {
        format: ExportFormat::Video {
            container: Container::Mkv,
            codec: VideoCodec::Hevc,
            audio: AudioCodec::Aac,
        },
        ..ExportSettings::default()
    };
    let path = export_with(project_at(Frame(0)), "timeline.mkv", mkv);
    assert_eq!(
        (video_codec(&path), audio_codec(&path)),
        ("hevc".into(), "aac".into())
    );
    let webm = ExportSettings {
        format: ExportFormat::Video {
            container: Container::WebM,
            codec: VideoCodec::Vp9,
            audio: AudioCodec::Opus,
        },
        quality: Quality::SMALL,
        ..ExportSettings::default()
    };
    let path = export_with(project_at(Frame(0)), "timeline.webm", webm);
    assert_eq!(
        (video_codec(&path), audio_codec(&path)),
        ("vp9".into(), "opus".into())
    );
    let duration = duration_of(&path);
    assert!((950_000..=1_100_000).contains(&duration), "{duration} µs");
}

#[test]
fn sound_alone_exports_the_mix() {
    let settings = ExportSettings {
        format: ExportFormat::Sound(AudioFormat::Mp3),
        ..ExportSettings::default()
    };
    let path = export_with(project_at(Frame(0)), "mix.mp3", settings);
    let info = probe(&path).unwrap();
    assert_eq!(info.streams.len(), 1);
    assert_eq!(audio_codec(&path), "mp3");
    let duration = duration_of(&path);
    assert!((950_000..=1_150_000).contains(&duration), "{duration} µs");
}

#[test]
fn a_timeline_without_sound_has_no_sound_to_export() {
    let mut project = project_at(Frame(0));
    let sound_track = project.sequence().tracks()[2].id();
    Command::SetTrackMuted(SetTrackMuted::new(sound_track, true))
        .apply(&mut project)
        .unwrap();
    let (engine, _events) = engine();
    let settings = ExportSettings {
        format: ExportFormat::Sound(AudioFormat::Wav),
        ..ExportSettings::default()
    };
    assert!(matches!(
        engine.export(Arc::new(project), output("silent.wav"), settings),
        Err(EngineError::NoSound)
    ));
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
    let _job = engine
        .export(Arc::new(project), path.clone(), ExportSettings::default())
        .unwrap();
    assert!(matches!(outcome(&events).0, ExportEvent::Finished { .. }));
    let duration = duration_of(&path);
    assert!((450_000..=600_000).contains(&duration), "{duration} µs");
}

#[test]
fn a_gap_exports_as_black() {
    let (engine, events) = engine();
    let path = output("gap.mp4");
    let _job = engine
        .export(
            Arc::new(project_at(Frame(15))),
            path.clone(),
            ExportSettings::default(),
        )
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
        .export(
            Arc::new(project_at(Frame(0))),
            path.clone(),
            ExportSettings::default(),
        )
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
    let _job = engine
        .export(Arc::new(empty), path.clone(), ExportSettings::default())
        .unwrap();
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
        .export(
            Arc::new(project_at(Frame(300))),
            path.clone(),
            ExportSettings::default(),
        )
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
        .export(
            Arc::new(project_at(Frame(0))),
            first,
            ExportSettings::default(),
        )
        .unwrap();
    let second = output("second.mp4");
    assert!(
        engine
            .export(
                Arc::new(project_at(Frame(0))),
                second.clone(),
                ExportSettings::default()
            )
            .is_err()
    );
    outcome(&events);
    assert!(!second.exists());
}
