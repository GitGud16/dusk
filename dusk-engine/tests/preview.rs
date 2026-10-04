//! The preview worker on the 8-bit sample (one second at 30 fps). Needs a graphics adapter;
//! CI runners use WARP.
#![cfg(feature = "gpu")]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use dusk_core::MediaTime;
use dusk_engine::{DECODER_IDLE, Gpu, Preview, PreviewEvent};
use dusk_render::Compositor;

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-h264-aac.mp4")
}

/// When frame `n` of the sample starts.
fn frame_time(n: i64) -> MediaTime {
    MediaTime((n * 2_000_000 + 30) / 60)
}

const PATIENCE: Duration = Duration::from_secs(30);

fn open(path: PathBuf) -> (Preview, mpsc::Receiver<PreviewEvent>, Gpu) {
    open_with_idle(path, DECODER_IDLE)
}

fn open_with_idle(path: PathBuf, idle: Duration) -> (Preview, mpsc::Receiver<PreviewEvent>, Gpu) {
    let gpu = Gpu::new().expect("a graphics adapter: hardware, or WARP on CI");
    let (sender, events) = mpsc::channel();
    let preview = Preview::open(&gpu, path, idle, move |event| {
        let _ = sender.send(event);
    })
    .expect("the worker starts");
    (preview, events, gpu)
}

#[test]
fn shows_the_frame_at_the_requested_time_and_size() {
    let (preview, events, gpu) = open(sample());
    preview.show(frame_time(15), (64, 48));
    match events.recv_timeout(PATIENCE).expect("an answer") {
        PreviewEvent::Frame { time, texture } => {
            assert_eq!(time, frame_time(15));
            assert_eq!((texture.width(), texture.height()), (64, 48));
            let rgba = Compositor::new(&gpu).read_rgba(&texture).unwrap();
            // testsrc2 is colorful; an all-black frame would mean nothing was drawn.
            assert!(rgba.chunks(4).any(|pixel| pixel[..3] != [0, 0, 0]));
        }
        other => panic!("expected a frame, got {other:?}"),
    }
}

#[test]
fn the_newest_request_is_always_served() {
    let (preview, events, _gpu) = open(sample());
    for n in 0..30 {
        preview.show(frame_time(n), (32, 24));
    }
    loop {
        match events
            .recv_timeout(PATIENCE)
            .expect("the last request is served")
        {
            PreviewEvent::Frame { time, .. } if time == frame_time(29) => break,
            PreviewEvent::Frame { .. } => {}
            other => panic!("expected frames, got {other:?}"),
        }
    }
}

#[test]
fn before_the_first_frame_nothing_is_shown() {
    let (preview, events, _gpu) = open(sample());
    preview.show(MediaTime(-1), (32, 24));
    assert!(matches!(
        events.recv_timeout(PATIENCE),
        Ok(PreviewEvent::Nothing { .. })
    ));
}

#[test]
fn a_file_that_cannot_be_opened_is_reported() {
    let (_preview, events, _gpu) = open(PathBuf::from("there is no such file.mp4"));
    assert!(matches!(
        events.recv_timeout(PATIENCE),
        Ok(PreviewEvent::Error(_))
    ));
}

#[test]
fn an_idle_decoder_is_closed_and_reopened_on_the_next_request() {
    let idle = Duration::from_millis(50);
    let (preview, events, _gpu) = open_with_idle(sample(), idle);
    preview.show(frame_time(3), (32, 24));
    assert!(matches!(
        events.recv_timeout(PATIENCE),
        Ok(PreviewEvent::Frame { .. })
    ));
    assert!(preview.is_decoding());

    std::thread::sleep(idle * 6);
    assert!(!preview.is_decoding(), "the decoder stayed open while idle");

    preview.show(frame_time(20), (32, 24));
    match events.recv_timeout(PATIENCE) {
        Ok(PreviewEvent::Frame { time, .. }) => assert_eq!(time, frame_time(20)),
        other => panic!("expected a frame, got {other:?}"),
    }
    assert!(preview.is_decoding());
}
