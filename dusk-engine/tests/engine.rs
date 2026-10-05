//! The engine on the 8-bit sample (one second at 30 fps, 320x240, with a tone), without
//! sound so the system clock drives playback. Needs a graphics adapter; CI runners use WARP.
#![cfg(feature = "gpu")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dusk_core::{Frame, Project, import};
use dusk_engine::{Engine, EngineEvent, EngineOptions, Gpu, media_info};
use dusk_render::Compositor;

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-h264-aac.mp4")
}

const PATIENCE: Duration = Duration::from_secs(30);

/// A project with the sample placed at `position`.
fn project_at(position: Frame) -> Arc<Project> {
    let info = media_info(&sample()).unwrap();
    let mut project = Project::new(info.frame_rate.unwrap(), (320, 240));
    import(&project, sample(), info, position)
        .apply(&mut project)
        .unwrap();
    Arc::new(project)
}

struct Running {
    engine: Engine,
    events: mpsc::Receiver<EngineEvent>,
    gpu: Gpu,
}

fn start_with(options: EngineOptions, project: Arc<Project>) -> Running {
    let gpu = Gpu::new().expect("a graphics adapter: hardware, or WARP on CI");
    let (sender, events) = mpsc::channel();
    let engine = Engine::new(&gpu, options, move |event| {
        let _ = sender.send(event);
    })
    .expect("the engine starts");
    engine.set_project(project);
    engine.set_preview_size((64, 48));
    Running {
        engine,
        events,
        gpu,
    }
}

fn start(project: Arc<Project>) -> Running {
    start_with(
        EngineOptions {
            sound: false,
            ..EngineOptions::default()
        },
        project,
    )
}

impl Running {
    /// The next frame event, skipping nothing.
    fn next_frame(&self) -> (Frame, Option<dusk_render::wgpu::Texture>) {
        match self.events.recv_timeout(PATIENCE).expect("an event") {
            EngineEvent::Frame { frame, texture } => (frame, texture),
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    /// Every frame shown until playback stops, and where it stopped.
    fn play_through(&self) -> (Vec<Frame>, Frame) {
        let mut frames = Vec::new();
        loop {
            match self.events.recv_timeout(PATIENCE).expect("an event") {
                EngineEvent::Frame { frame, .. } => frames.push(frame),
                EngineEvent::Stopped { frame } => return (frames, frame),
                EngineEvent::Error(error) => panic!("{error}"),
                EngineEvent::Export(event) => panic!("no export was started: {event:?}"),
            }
        }
    }
}

#[test]
fn shows_the_requested_frame_at_the_preview_size() {
    let running = start(project_at(Frame(0)));
    running.engine.show(Frame(15));
    let (frame, texture) = running.next_frame();
    assert_eq!(frame, Frame(15));
    let texture = texture.expect("a picture");
    assert_eq!((texture.width(), texture.height()), (64, 48));
    let rgba = Compositor::new(&running.gpu).read_rgba(&texture).unwrap();
    // testsrc2 is colorful; an all-black frame would mean nothing was drawn.
    assert!(rgba.chunks(4).any(|pixel| pixel[..3] != [0, 0, 0]));
}

#[test]
fn a_gap_is_a_black_frame() {
    let running = start(project_at(Frame(15)));
    running.engine.show(Frame(5));
    let (frame, texture) = running.next_frame();
    assert_eq!(frame, Frame(5));
    let texture = texture.expect("a frame");
    assert_eq!((texture.width(), texture.height()), (64, 48));
    let rgba = Compositor::new(&running.gpu).read_rgba(&texture).unwrap();
    assert!(rgba.chunks(4).all(|pixel| pixel == [0, 0, 0, 255]));
}

#[test]
fn the_preview_has_the_shape_of_the_sequence() {
    // A portrait sequence in a landscape preview: the frame is portrait, the bars around it
    // are the preview's own background.
    let info = media_info(&sample()).unwrap();
    let mut project = Project::new(info.frame_rate.unwrap(), (240, 320));
    import(&project, sample(), info, Frame(0))
        .apply(&mut project)
        .unwrap();
    let running = start(Arc::new(project));
    running.engine.show(Frame(5));
    let (_, texture) = running.next_frame();
    let texture = texture.expect("a picture");
    assert_eq!((texture.width(), texture.height()), (36, 48));
}

#[test]
fn a_photo_shows_for_as_long_as_its_clip_lasts() {
    // Stored landscape with EXIF orientation 6: shown upright, a portrait picture.
    let photo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/photo-turned.jpg");
    let info = media_info(&photo).unwrap();
    assert_eq!((info.width, info.height), (240, 320));
    let mut project = Project::new(dusk_core::Rational::new(30, 1).unwrap(), (240, 320));
    import(&project, photo, info, Frame(0))
        .apply(&mut project)
        .unwrap();
    let running = start(Arc::new(project));
    for at in [0, 100, 149] {
        running.engine.show(Frame(at));
        let (frame, texture) = running.next_frame();
        assert_eq!(frame, Frame(at));
        let texture = texture.expect("a picture");
        assert_eq!((texture.width(), texture.height()), (36, 48));
        let rgba = Compositor::new(&running.gpu).read_rgba(&texture).unwrap();
        // The test pattern fills the portrait frame: no black bars on either side.
        let lit = |x: usize| (0..48).any(|y| rgba[(y * 36 + x) * 4..][..3] != [0, 0, 0]);
        assert!(lit(1) && lit(34));
    }
}

#[test]
fn a_scrub_ends_on_the_exact_frame() {
    let running = start(project_at(Frame(0)));
    for frame in 0..30 {
        running.engine.scrub(Frame(frame));
    }
    running.engine.show(Frame(29));
    loop {
        let (frame, texture) = running.next_frame();
        if frame == Frame(29) && texture.is_some() {
            break;
        }
    }
}

#[test]
fn playback_runs_to_the_end_in_real_time() {
    let running = start(project_at(Frame(0)));
    let started = Instant::now();
    running.engine.play(Frame(0), 1.0);
    let (frames, stopped) = running.play_through();
    let took = started.elapsed();
    assert_eq!(stopped, Frame(29));
    assert!(
        frames.windows(2).all(|pair| pair[0] < pair[1]),
        "{frames:?}"
    );
    assert!(frames.len() > 15, "only {} frames shown", frames.len());
    assert!(
        took >= Duration::from_millis(900) && took < Duration::from_secs(3),
        "{took:?}"
    );
    assert_eq!(running.engine.playing(), None);
}

#[test]
fn double_speed_takes_half_the_time() {
    let running = start(project_at(Frame(0)));
    let started = Instant::now();
    running.engine.play(Frame(0), 2.0);
    let (_, stopped) = running.play_through();
    let took = started.elapsed();
    assert_eq!(stopped, Frame(29));
    assert!(
        took >= Duration::from_millis(400) && took < Duration::from_millis(1500),
        "{took:?}"
    );
}

#[test]
fn fast_playback_steps_through_keyframes_to_the_end() {
    let running = start(project_at(Frame(0)));
    running.engine.play(Frame(0), 32.0);
    let (_, stopped) = running.play_through();
    assert_eq!(stopped, Frame(29));
}

#[test]
fn fast_playback_backwards_stops_at_the_start() {
    let running = start(project_at(Frame(0)));
    running.engine.play(Frame(29), -8.0);
    let (_, stopped) = running.play_through();
    assert_eq!(stopped, Frame(0));
}

#[test]
fn playing_backwards_stops_at_the_start() {
    let running = start(project_at(Frame(0)));
    running.engine.play(Frame(20), -1.0);
    let (frames, stopped) = running.play_through();
    assert_eq!(stopped, Frame(0));
    assert!(
        frames.windows(2).all(|pair| pair[0] > pair[1]),
        "{frames:?}"
    );
}

#[test]
fn pausing_stops_where_the_clock_is() {
    let running = start(project_at(Frame(0)));
    running.engine.play(Frame(0), 1.0);
    std::thread::sleep(Duration::from_millis(300));
    let paused = running.engine.pause().expect("it was playing");
    // About 9 frames in, give or take a slow machine.
    assert!((Frame(4)..=Frame(20)).contains(&paused), "{paused:?}");
    assert_eq!(running.engine.playing(), None);
    assert_eq!(running.engine.pause(), None);
    // The preview ends on the frame playback stopped at.
    let mut last = None;
    while let Ok(event) = running.events.recv_timeout(Duration::from_millis(500)) {
        if let EngineEvent::Frame { frame, .. } = event {
            last = Some(frame);
        }
    }
    assert_eq!(last, Some(paused));
}

#[test]
fn an_unused_decoder_is_closed() {
    let running = start_with(
        EngineOptions {
            sound: false,
            decoder_idle: Duration::from_millis(200),
            ..EngineOptions::default()
        },
        project_at(Frame(0)),
    );
    running.engine.show(Frame(3));
    running.next_frame();
    assert_eq!(running.engine.open_decoders(), 1);
    let deadline = Instant::now() + PATIENCE;
    while running.engine.open_decoders() > 0 {
        assert!(Instant::now() < deadline, "the decoder stays open");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// With sound the audio device drives the clock; on a machine without one (CI) playback
/// runs silently on the system clock instead.
#[test]
fn playback_with_sound_runs_to_the_end_in_real_time() {
    let running = start_with(EngineOptions::default(), project_at(Frame(0)));
    let started = Instant::now();
    running.engine.play(Frame(0), 1.0);
    let stopped = loop {
        match running.events.recv_timeout(PATIENCE).expect("an event") {
            EngineEvent::Frame { .. } => {}
            EngineEvent::Stopped { frame } => break frame,
            // No usable device here; playback carries on without sound.
            EngineEvent::Error(error) => eprintln!("{error}"),
            EngineEvent::Export(event) => panic!("no export was started: {event:?}"),
        }
    };
    let took = started.elapsed();
    assert_eq!(stopped, Frame(29));
    assert!(
        took >= Duration::from_millis(900) && took < Duration::from_secs(3),
        "{took:?}"
    );
}
