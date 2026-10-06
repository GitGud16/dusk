//! The engine on the 8-bit sample (one second at 30 fps, 320x240, with a tone), without
//! sound so the system clock drives playback. Needs a graphics adapter; CI runners use WARP.
#![cfg(feature = "gpu")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use dusk_core::{Command, Frame, MediaId, Project, RelinkMedia, RemoveClips, import, split_at};
use dusk_engine::{Engine, EngineEvent, EngineOptions, Gpu, Preview, media_info};
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

/// The sample's first third, then its last third straight after it: one file shown from two
/// places, so going from one to the other means a jump in the file.
fn project_with_a_jump() -> Arc<Project> {
    let mut project = (*project_at(Frame(0))).clone();
    for at in [Frame(10), Frame(20)] {
        split_at(&project, at, None)
            .unwrap()
            .apply(&mut project)
            .unwrap();
    }
    let middle = project.sequence().visible_video_at(Frame(15)).unwrap().id;
    Command::RemoveClips(RemoveClips::new(middle, true))
        .apply(&mut project)
        .unwrap();
    Arc::new(project)
}

/// The engines of these tests run one at a time: the checks of playback in real time need
/// the machine to themselves, and CI runners draw with WARP, on their few cores.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

struct Running {
    engine: Engine,
    events: mpsc::Receiver<EngineEvent>,
    gpu: Gpu,
    /// Dropped last, once the engine has stopped.
    _turn: MutexGuard<'static, ()>,
}

fn start_with(options: EngineOptions, project: Arc<Project>) -> Running {
    // A test that failed holding the turn leaves nothing behind that matters here.
    let turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let gpu = Gpu::new().expect("a graphics adapter: hardware, or WARP on CI");
    let (sender, events) = mpsc::channel();
    let engine = Engine::new(&gpu, options, move |event| {
        let _ = sender.send(event);
    })
    .expect("the engine starts");
    engine.set_project(Preview::Main, project);
    engine.set_preview_size(Preview::Main, (64, 48));
    Running {
        engine,
        events,
        gpu,
        _turn: turn,
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
            EngineEvent::Frame { frame, texture, .. } => (frame, texture),
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    /// Every frame shown until playback stops, and where it stopped.
    fn play_through(&self) -> (Vec<Frame>, Frame) {
        let mut frames = Vec::new();
        loop {
            match self.events.recv_timeout(PATIENCE).expect("an event") {
                EngineEvent::Frame { frame, .. } => frames.push(frame),
                EngineEvent::Stopped { frame, .. } => return (frames, frame),
                EngineEvent::Error(error) => panic!("{error}"),
                EngineEvent::Export(event) => panic!("no export was started: {event:?}"),
                EngineEvent::Thumbnail { media, .. } => {
                    panic!("no thumbnail was asked for: {media:?}")
                }
            }
        }
    }
}

#[test]
fn the_next_clip_gets_a_decoder_of_its_own_before_it_comes_into_view() {
    let running = start(project_with_a_jump());
    running.engine.play(Preview::Main, Frame(0), 1.0);
    let mut most_while_first = 0;
    let stopped = loop {
        match running.events.recv_timeout(PATIENCE).expect("an event") {
            EngineEvent::Frame { frame, .. } => {
                let open = running.engine.open_decoders();
                assert!(open <= 2, "{open} video decoders open");
                if frame < Frame(10) {
                    most_while_first = most_while_first.max(open);
                }
            }
            EngineEvent::Stopped { frame, .. } => break frame,
            EngineEvent::Error(error) => panic!("{error}"),
            EngineEvent::Export(event) => panic!("no export was started: {event:?}"),
            EngineEvent::Thumbnail { media, .. } => panic!("no thumbnail was asked for: {media:?}"),
        }
    };
    assert_eq!(stopped, Frame(19));
    // The first clip's decoder, and the second's, moved to its first frame meanwhile.
    assert_eq!(most_while_first, 2);
}

#[test]
fn thumbnails_come_from_the_thumbnail_thread() {
    let running = start(project_at(Frame(0)));
    let info = media_info(&sample()).unwrap();
    running.engine.make_thumbnail(MediaId(7), sample(), info);
    let thumbnail = loop {
        match running.events.recv_timeout(PATIENCE).expect("an event") {
            EngineEvent::Thumbnail {
                media,
                path,
                thumbnail,
            } => {
                // Which media, and the file it was made from.
                assert_eq!((media, path), (MediaId(7), sample()));
                break thumbnail;
            }
            EngineEvent::Frame { .. } => {}
            other => panic!("expected a thumbnail, got {other:?}"),
        }
    };
    assert_eq!((thumbnail.width, thumbnail.height), (128, 96));
}

/// The next frame event, with the preview it was drawn for.
fn next_frame_of(running: &Running) -> (Preview, Frame, Option<dusk_render::wgpu::Texture>) {
    loop {
        match running.events.recv_timeout(PATIENCE).expect("an event") {
            EngineEvent::Frame {
                preview,
                frame,
                texture,
            } => return (preview, frame, texture),
            EngineEvent::Error(error) => panic!("{error}"),
            _ => {}
        }
    }
}

fn is_black(running: &Running, texture: &dusk_render::wgpu::Texture) -> bool {
    let rgba = Compositor::new(&running.gpu).read_rgba(texture).unwrap();
    rgba.chunks(4).all(|pixel| pixel[..3] == [0, 0, 0])
}

#[test]
fn the_clip_editor_preview_draws_a_project_of_its_own() {
    // The main window's sample starts at frame 0; the clip editor's at frame 15, so its
    // frame 5 is a gap.
    let running = start(project_at(Frame(0)));
    running
        .engine
        .set_project(Preview::ClipEditor, project_at(Frame(15)));
    running
        .engine
        .set_preview_size(Preview::ClipEditor, (32, 24));
    running.engine.show(Preview::Main, Frame(5));
    let (preview, frame, texture) = next_frame_of(&running);
    let texture = texture.expect("a picture");
    assert_eq!((preview, frame), (Preview::Main, Frame(5)));
    assert_eq!((texture.width(), texture.height()), (64, 48));
    assert!(!is_black(&running, &texture));
    running.engine.show(Preview::ClipEditor, Frame(5));
    let (preview, frame, texture) = next_frame_of(&running);
    let texture = texture.expect("a frame");
    assert_eq!((preview, frame), (Preview::ClipEditor, Frame(5)));
    assert_eq!((texture.width(), texture.height()), (32, 24));
    assert!(is_black(&running, &texture));
}

#[test]
fn playing_in_one_preview_stops_playing_in_the_other() {
    let running = start(project_at(Frame(0)));
    running
        .engine
        .set_project(Preview::ClipEditor, project_at(Frame(0)));
    running
        .engine
        .set_preview_size(Preview::ClipEditor, (32, 24));
    running.engine.play(Preview::Main, Frame(0), 1.0);
    running.engine.play(Preview::ClipEditor, Frame(20), 1.0);
    assert_eq!(running.engine.playing(), Some((Preview::ClipEditor, 1.0)));
    // From here on only the clip editor's frames come, up to the end of its sequence.
    let stopped = loop {
        match running.events.recv_timeout(PATIENCE).expect("an event") {
            EngineEvent::Frame { preview, frame, .. } if frame >= Frame(20) => {
                assert_eq!(preview, Preview::ClipEditor)
            }
            EngineEvent::Frame { .. } => {}
            EngineEvent::Stopped { preview, frame } => break (preview, frame),
            EngineEvent::Error(error) => panic!("{error}"),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(stopped, (Preview::ClipEditor, Frame(29)));
}

#[test]
fn a_closed_preview_draws_nothing_more() {
    let running = start(project_at(Frame(0)));
    running
        .engine
        .set_project(Preview::ClipEditor, project_at(Frame(0)));
    running
        .engine
        .set_preview_size(Preview::ClipEditor, (32, 24));
    running.engine.show(Preview::ClipEditor, Frame(5));
    assert_eq!(next_frame_of(&running).0, Preview::ClipEditor);
    running.engine.close_preview(Preview::ClipEditor);
    running.engine.show(Preview::ClipEditor, Frame(6));
    running.engine.show(Preview::Main, Frame(7));
    // Requests are taken in order: had the closed preview drawn, its frame would come first.
    let (preview, frame, _) = next_frame_of(&running);
    assert_eq!((preview, frame), (Preview::Main, Frame(7)));
}

#[test]
fn shows_the_requested_frame_at_the_preview_size() {
    let running = start(project_at(Frame(0)));
    running.engine.show(Preview::Main, Frame(15));
    let (frame, texture) = running.next_frame();
    assert_eq!(frame, Frame(15));
    let texture = texture.expect("a picture");
    assert_eq!((texture.width(), texture.height()), (64, 48));
    let rgba = Compositor::new(&running.gpu).read_rgba(&texture).unwrap();
    // testsrc2 is colorful; an all-black frame would mean nothing was drawn.
    assert!(rgba.chunks(4).any(|pixel| pixel[..3] != [0, 0, 0]));
}

#[test]
fn a_relinked_file_never_shows_the_old_files_frames() {
    let running = start(project_at(Frame(0)));
    running.engine.show(Preview::Main, Frame(5));
    let (_, texture) = running.next_frame();
    assert!(!is_black(&running, &texture.expect("a picture")));
    // The same media, now where no file is: the frames decoded from its old place must not
    // stand in for it, so the frame fails to open.
    let mut moved = (*project_at(Frame(0))).clone();
    let media = moved.media()[0].clone();
    let elsewhere = sample().with_file_name("moved-away.mp4");
    Command::RelinkMedia(RelinkMedia::new(media.id, elsewhere, media.info))
        .apply(&mut moved)
        .unwrap();
    running.engine.set_project(Preview::Main, Arc::new(moved));
    running.engine.show(Preview::Main, Frame(5));
    loop {
        match running.events.recv_timeout(PATIENCE).expect("an event") {
            EngineEvent::Error(_) => break,
            EngineEvent::Frame { texture, .. } => {
                panic!("a frame came instead of the error: {texture:?}")
            }
            _ => {}
        }
    }
    // Then the frame shows black, as a clip whose file is missing does, rather than the
    // picture that was on screen before.
    let (frame, texture) = running.next_frame();
    assert_eq!(frame, Frame(5));
    assert!(is_black(&running, &texture.expect("a frame")));
}

#[test]
fn a_media_id_given_to_another_file_never_shows_the_old_files_frames() {
    let running = start(project_at(Frame(0)));
    running.engine.show(Preview::Main, Frame(5));
    let (_, texture) = running.next_frame();
    assert!(!is_black(&running, &texture.expect("a picture")));
    let info = media_info(&sample()).unwrap();
    // The media goes (an import undone), and a new one is given its id: here a file that is
    // not there, so a frame of the old file would be the only picture it could show.
    let empty = Project::new(info.frame_rate.unwrap(), (320, 240));
    running
        .engine
        .set_project(Preview::Main, Arc::new(empty.clone()));
    running.engine.show(Preview::Main, Frame(5));
    let (_, texture) = running.next_frame();
    assert!(is_black(&running, &texture.expect("a frame")));
    let mut other = empty;
    import(
        &other,
        sample().with_file_name("another-file.mp4"),
        info,
        Frame(0),
    )
    .apply(&mut other)
    .unwrap();
    assert_eq!(
        other.media()[0].id,
        project_at(Frame(0)).media()[0].id,
        "the new media has the old one's id"
    );
    running.engine.set_project(Preview::Main, Arc::new(other));
    running.engine.show(Preview::Main, Frame(5));
    loop {
        match running.events.recv_timeout(PATIENCE).expect("an event") {
            EngineEvent::Error(_) => break,
            EngineEvent::Frame { texture, .. } => {
                panic!("a frame came instead of the error: {texture:?}")
            }
            _ => {}
        }
    }
}

#[test]
fn a_gap_is_a_black_frame() {
    let running = start(project_at(Frame(15)));
    running.engine.show(Preview::Main, Frame(5));
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
    running.engine.show(Preview::Main, Frame(5));
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
        running.engine.show(Preview::Main, Frame(at));
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
        running.engine.scrub(Preview::Main, Frame(frame));
    }
    running.engine.show(Preview::Main, Frame(29));
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
    running.engine.play(Preview::Main, Frame(0), 1.0);
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
    running.engine.play(Preview::Main, Frame(0), 2.0);
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
    running.engine.play(Preview::Main, Frame(0), 32.0);
    let (_, stopped) = running.play_through();
    assert_eq!(stopped, Frame(29));
}

#[test]
fn fast_playback_backwards_stops_at_the_start() {
    let running = start(project_at(Frame(0)));
    running.engine.play(Preview::Main, Frame(29), -8.0);
    let (_, stopped) = running.play_through();
    assert_eq!(stopped, Frame(0));
}

#[test]
fn playing_backwards_with_a_small_cache_still_reaches_the_start() {
    // Room for five of the sample's 115 KB frames, so half of it holds two: its one group of
    // pictures is decoded again from the keyframe every two frames.
    let options = EngineOptions {
        sound: false,
        cache_cap: 600_000,
        ..EngineOptions::default()
    };
    let running = start_with(options, project_at(Frame(0)));
    running.engine.play(Preview::Main, Frame(29), -1.0);
    let (frames, stopped) = running.play_through();
    assert_eq!(stopped, Frame(0));
    assert!(
        frames.windows(2).all(|pair| pair[0] > pair[1]),
        "{frames:?}"
    );
    assert!(frames.len() > 10, "only {} frames shown", frames.len());
}

#[test]
fn playing_backwards_stops_at_the_start() {
    let running = start(project_at(Frame(0)));
    running.engine.play(Preview::Main, Frame(20), -1.0);
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
    running.engine.play(Preview::Main, Frame(0), 1.0);
    std::thread::sleep(Duration::from_millis(300));
    let (preview, paused) = running.engine.pause().expect("it was playing");
    assert_eq!(preview, Preview::Main);
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
    running.engine.show(Preview::Main, Frame(3));
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
    running.engine.play(Preview::Main, Frame(0), 1.0);
    let stopped = loop {
        match running.events.recv_timeout(PATIENCE).expect("an event") {
            EngineEvent::Frame { .. } => {}
            EngineEvent::Stopped { frame, .. } => break frame,
            // No usable device here; playback carries on without sound.
            EngineEvent::Error(error) => eprintln!("{error}"),
            EngineEvent::Export(event) => panic!("no export was started: {event:?}"),
            EngineEvent::Thumbnail { media, .. } => panic!("no thumbnail was asked for: {media:?}"),
        }
    };
    let took = started.elapsed();
    assert_eq!(stopped, Frame(29));
    assert!(
        took >= Duration::from_millis(900) && took < Duration::from_secs(3),
        "{took:?}"
    );
}
