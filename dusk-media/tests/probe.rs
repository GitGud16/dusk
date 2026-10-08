//! M0 smoke test: FFmpeg is linked and loading, and a sample file's streams can be read.

use std::path::{Path, PathBuf};

use dusk_media::{MediaError, StreamDetail, StreamKind, probe};

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-h264-aac.mp4")
}

#[test]
fn probe_reads_the_video_and_audio_streams_of_the_sample() {
    let info = probe(&sample()).expect("the sample MP4 should probe");
    println!("{}: {info}", sample().display());

    assert_eq!(info.streams.len(), 2);

    let video = &info.streams[0];
    assert_eq!(video.kind, StreamKind::Video);
    assert_eq!(video.codec, "h264");
    match video.detail {
        StreamDetail::Video {
            width,
            height,
            frame_rate,
            base_frame_rate,
            cover_art,
            orientation,
            bit_depth,
        } => {
            assert_eq!((width, height), (320, 240));
            assert_eq!(bit_depth, 8);
            assert_eq!(frame_rate, Some((30, 1)));
            // A constant-rate file: the base rate equals the average.
            assert_eq!(base_frame_rate, Some((30, 1)));
            assert!(!cover_art);
            assert_eq!(orientation, dusk_core::Orientation::UPRIGHT);
        }
        ref other => panic!("expected video details, got {other:?}"),
    }

    let audio = &info.streams[1];
    assert_eq!(audio.kind, StreamKind::Audio);
    assert_eq!(audio.codec, "aac");
    match audio.detail {
        StreamDetail::Audio {
            sample_rate,
            channels,
        } => {
            assert_eq!((sample_rate, channels), (48_000, 1));
        }
        ref other => panic!("expected audio details, got {other:?}"),
    }
}

#[test]
fn probe_reads_the_bit_depth_of_10_bit_video() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-vp9-10bit.webm");
    let info = probe(&path).unwrap();
    match info.streams[0].detail {
        StreamDetail::Video { bit_depth, .. } => assert_eq!(bit_depth, 10),
        ref other => panic!("expected video details, got {other:?}"),
    }
}

#[test]
fn probe_reports_the_duration_in_microseconds() {
    let info = probe(&sample()).expect("the sample MP4 should probe");
    let duration = info.duration_us.expect("the sample has a known duration");
    assert!(
        (900_000..=1_100_000).contains(&duration),
        "expected about one second, got {duration} us"
    );
}

#[test]
fn probe_of_a_missing_file_names_the_file() {
    let err = probe(Path::new("no-such-file.mp4")).expect_err("a missing file must not probe");
    assert!(matches!(err, MediaError::NotAFile { .. }), "got {err:?}");
    assert!(
        err.to_string().contains("no-such-file.mp4"),
        "message was: {err}"
    );
}

#[test]
fn probe_refuses_urls() {
    // Dusk has no network code (CLAUDE.md): a URL must never reach FFmpeg's network
    // protocols, even if one is passed where a path is expected.
    let err = probe(Path::new("http://127.0.0.1:9/clip.mp4")).expect_err("a URL must not probe");
    assert!(matches!(err, MediaError::NotAFile { .. }), "got {err:?}");
}

#[test]
fn each_stream_prints_as_one_readable_line() {
    let info = probe(&sample()).expect("the sample MP4 should probe");
    assert_eq!(
        info.streams[0].to_string(),
        "#0 video h264 320x240 30/1 fps"
    );
    assert_eq!(info.streams[1].to_string(), "#1 audio aac 48000 Hz 1 ch");
}

#[test]
fn a_video_shot_upright_on_a_phone_says_how_to_turn_it() {
    // Stored landscape with a display matrix turning it a quarter clockwise, as a portrait
    // phone video is (ffprobe: rotation -90).
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-rotated.mp4");
    let info = probe(&path).unwrap();
    let video = info
        .streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Video)
        .unwrap();
    match video.detail {
        StreamDetail::Video {
            width,
            height,
            orientation,
            ..
        } => {
            assert_eq!((width, height), (320, 240));
            assert_eq!(orientation, dusk_core::Orientation::new(1, false));
        }
        ref other => panic!("expected video details, got {other:?}"),
    }
}
