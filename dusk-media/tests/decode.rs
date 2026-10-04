//! Decoding the sample: one second of `testsrc2` at 320x240 and 30 fps, so frame n starts at
//! n/30 s.

use std::path::{Path, PathBuf};

use dusk_core::{ColorMatrix, ColorRange, MediaTime, PictureLayout};
use dusk_media::{Acceleration, MediaError, VideoDecoder};

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-h264-aac.mp4")
}

/// When frame `n` starts, rounded to the microsecond.
fn frame_time(n: i64) -> MediaTime {
    MediaTime((n * 2_000_000 + 30) / 60)
}

fn decoder() -> VideoDecoder {
    VideoDecoder::open(&sample(), Acceleration::Auto).expect("the sample opens")
}

fn time_at(decoder: &mut VideoDecoder, time: MediaTime) -> MediaTime {
    decoder
        .frame_at(time)
        .expect("decoding works")
        .expect("a frame is shown")
        .time
}

#[test]
fn the_first_frame_is_a_whole_nv12_picture() {
    let frame = decoder().frame_at(MediaTime(0)).unwrap().unwrap();
    assert_eq!(frame.time, MediaTime(0));
    let picture = &frame.picture;
    assert_eq!(
        (picture.width, picture.height, picture.layout),
        (320, 240, PictureLayout::Nv12)
    );
    assert_eq!(picture.luma.len(), 320 * 240);
    assert_eq!(picture.chroma.len(), 160 * 120 * 2);
}

#[test]
fn a_time_between_frames_shows_the_earlier_frame() {
    let mut decoder = decoder();
    assert_eq!(time_at(&mut decoder, frame_time(15)), frame_time(15));
    assert_eq!(time_at(&mut decoder, MediaTime(510_000)), frame_time(15));
    assert_eq!(time_at(&mut decoder, MediaTime(533_000)), frame_time(15));
    assert_eq!(time_at(&mut decoder, frame_time(16)), frame_time(16));
}

#[test]
fn every_frame_is_reached_in_order() {
    let mut decoder = decoder();
    for n in 0..30 {
        assert_eq!(
            time_at(&mut decoder, frame_time(n)),
            frame_time(n),
            "frame {n}"
        );
    }
}

#[test]
fn going_back_finds_the_earlier_frame() {
    let mut decoder = decoder();
    assert_eq!(time_at(&mut decoder, MediaTime(900_000)), frame_time(27));
    assert_eq!(time_at(&mut decoder, frame_time(3)), frame_time(3));
}

#[test]
fn the_picture_changes_over_time() {
    let mut decoder = decoder();
    let first = decoder.frame_at(MediaTime(0)).unwrap().unwrap().picture;
    let later = decoder.frame_at(frame_time(15)).unwrap().unwrap().picture;
    assert_ne!(first.luma, later.luma);
}

#[test]
fn the_last_frame_holds_after_the_stream_ends() {
    let mut decoder = decoder();
    assert_eq!(time_at(&mut decoder, MediaTime(5_000_000)), frame_time(29));
}

#[test]
fn nothing_is_shown_before_the_first_frame() {
    assert!(decoder().frame_at(MediaTime(-1)).unwrap().is_none());
}

#[test]
fn an_untagged_sd_picture_uses_bt601_in_limited_range() {
    let picture = decoder().frame_at(MediaTime(0)).unwrap().unwrap().picture;
    assert_eq!(
        (picture.matrix, picture.range),
        (ColorMatrix::Bt601, ColorRange::Limited)
    );
}

#[test]
fn software_decoding_never_uses_the_hardware() {
    let mut decoder = VideoDecoder::open(&sample(), Acceleration::Software).unwrap();
    decoder.frame_at(MediaTime(0)).unwrap();
    assert!(!decoder.is_hardware());
}

/// H.264 decoding is bit-exact, so a hardware decoder must produce the software picture.
/// Machines without a hardware decoder (CI runners) only check that `Auto` fell back.
#[test]
fn hardware_and_software_decoding_agree() {
    let mut auto = decoder();
    let mut software = VideoDecoder::open(&sample(), Acceleration::Software).unwrap();
    let from_auto = auto.frame_at(frame_time(10)).unwrap().unwrap();
    let from_software = software.frame_at(frame_time(10)).unwrap().unwrap();
    if !auto.is_hardware() {
        eprintln!("no hardware decoder on this machine; Auto fell back to software");
    }
    assert_eq!(from_auto.time, from_software.time);
    assert!(
        from_auto.picture == from_software.picture,
        "the pictures differ"
    );
}

#[test]
fn a_file_without_video_is_refused() {
    let readme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/README.md");
    assert!(matches!(
        VideoDecoder::open(&readme, Acceleration::Auto),
        Err(MediaError::Open { .. } | MediaError::NoVideo { .. })
    ));
}

#[test]
fn a_decoder_can_move_to_a_worker_thread() {
    fn assert_send<T: Send>() {}
    assert_send::<VideoDecoder>();
}

fn sample_10_bit() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-vp9-10bit.webm")
}

#[test]
fn a_10_bit_video_decodes_to_p010_with_the_value_in_the_top_bits() {
    let mut decoder = VideoDecoder::open(&sample_10_bit(), Acceleration::Software).unwrap();
    let picture = decoder.frame_at(MediaTime(0)).unwrap().unwrap().picture;
    assert_eq!(picture.layout, PictureLayout::P010);
    assert_eq!(picture.luma.len(), 320 * 240 * 2);
    assert_eq!(picture.chroma.len(), 160 * 120 * 2 * 2);
    let low_bits_clear = |plane: &[u8]| {
        plane
            .as_chunks::<2>()
            .0
            .iter()
            .all(|sample| sample[0] & 0x3f == 0)
    };
    assert!(low_bits_clear(&picture.luma) && low_bits_clear(&picture.chroma));
    assert!(
        picture
            .luma
            .as_chunks::<2>()
            .0
            .iter()
            .any(|sample| sample[1] != 0)
    );
}

/// VP9 decoding is bit-exact too; this also covers the hardware P010 path where it exists.
#[test]
fn hardware_and_software_decoding_agree_on_10_bit_video() {
    let mut auto = VideoDecoder::open(&sample_10_bit(), Acceleration::Auto).unwrap();
    let mut software = VideoDecoder::open(&sample_10_bit(), Acceleration::Software).unwrap();
    let from_auto = auto.frame_at(frame_time(7)).unwrap().unwrap();
    let from_software = software.frame_at(frame_time(7)).unwrap().unwrap();
    if !auto.is_hardware() {
        eprintln!("no hardware VP9 decoder on this machine; Auto fell back to software");
    }
    assert_eq!(from_auto.time, from_software.time);
    assert!(
        from_auto.picture == from_software.picture,
        "the pictures differ"
    );
}
