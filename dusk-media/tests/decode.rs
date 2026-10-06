//! Decoding the sample: one second of `testsrc2` at 320x240 and 30 fps, so frame n starts at
//! n/30 s.

use std::path::{Path, PathBuf};

use dusk_core::color::{Primaries, Transfer};
use dusk_core::{ColorMatrix, ColorRange, MediaTime, PictureLayout};
use dusk_media::{Acceleration, Following, MediaError, Step, VideoDecoder};

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-h264-aac.mp4")
}

/// When frame `n` starts, rounded to the microsecond.
fn frame_time(n: i64) -> MediaTime {
    MediaTime((n * 2_000_000 + 30) / 60)
}

fn decoder() -> VideoDecoder {
    VideoDecoder::open(&sample(), Acceleration::Software).expect("the sample opens")
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
/// Machines without a hardware decoder (CI runners) only check that it fell back.
#[test]
fn hardware_and_software_decoding_agree() {
    let mut hardware = VideoDecoder::open(&sample(), Acceleration::Hardware).unwrap();
    let mut software = VideoDecoder::open(&sample(), Acceleration::Software).unwrap();
    let from_hardware = hardware.frame_at(frame_time(10)).unwrap().unwrap();
    let from_software = software.frame_at(frame_time(10)).unwrap().unwrap();
    if !hardware.is_hardware() {
        eprintln!("no hardware decoder on this machine; decoding fell back to software");
    }
    assert_eq!(from_hardware.time, from_software.time);
    assert!(
        from_hardware.picture == from_software.picture,
        "the pictures differ"
    );
}

#[test]
fn a_file_without_video_is_refused() {
    let readme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/README.md");
    assert!(matches!(
        VideoDecoder::open(&readme, Acceleration::Software),
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
    let mut hardware = VideoDecoder::open(&sample_10_bit(), Acceleration::Hardware).unwrap();
    let mut software = VideoDecoder::open(&sample_10_bit(), Acceleration::Software).unwrap();
    let from_hardware = hardware.frame_at(frame_time(7)).unwrap().unwrap();
    let from_software = software.frame_at(frame_time(7)).unwrap().unwrap();
    if !hardware.is_hardware() {
        eprintln!("no hardware VP9 decoder on this machine; decoding fell back to software");
    }
    assert_eq!(from_hardware.time, from_software.time);
    assert!(
        from_hardware.picture == from_software.picture,
        "the pictures differ"
    );
}

#[test]
fn frames_come_one_after_another_from_a_seek() {
    let mut decoder = decoder();
    decoder.seek(frame_time(10)).unwrap();
    // The sample has one keyframe, at frame 0, so decoding restarts there.
    let times: Vec<_> = (0..12)
        .map(|_| decoder.next_frame().unwrap().expect("a frame").time)
        .collect();
    let expected: Vec<_> = (0..12).map(frame_time).collect();
    assert_eq!(times, expected);
}

#[test]
fn stepping_decodes_one_frame_at_a_time_to_what_frame_at_finds() {
    let mut stepped = decoder();
    stepped.seek(frame_time(12)).unwrap();
    let mut steps = 0;
    let frame = loop {
        match stepped.step_to(frame_time(12)).unwrap() {
            Step::Working => steps += 1,
            Step::Done(frame) => break frame.expect("a frame"),
        }
    };
    // From the keyframe at frame 0 up to frame 13, which shows frame 12 is the one wanted.
    assert_eq!(steps, 14);
    assert_eq!(stepped.following(), Following::Next(frame_time(13)));
    let expected = decoder().frame_at(frame_time(12)).unwrap().unwrap();
    assert_eq!(frame.time, expected.time);
    assert_eq!(frame.picture.luma, expected.picture.luma);
    assert_eq!(frame.picture.chroma, expected.picture.chroma);
}

#[test]
fn next_frame_continues_after_frame_at() {
    let mut decoder = decoder();
    assert_eq!(time_at(&mut decoder, frame_time(5)), frame_time(5));
    assert_eq!(decoder.next_frame().unwrap().unwrap().time, frame_time(6));
    assert_eq!(decoder.next_frame().unwrap().unwrap().time, frame_time(7));
    assert_eq!(time_at(&mut decoder, frame_time(9)), frame_time(9));
}

#[test]
fn next_frame_ends_after_the_last_frame() {
    let mut decoder = decoder();
    assert_eq!(time_at(&mut decoder, frame_time(29)), frame_time(29));
    assert!(decoder.next_frame().unwrap().is_none());
}

#[test]
fn the_decoder_knows_when_the_next_frame_starts() {
    let mut decoder = decoder();
    decoder.frame_at(frame_time(5)).unwrap();
    assert_eq!(decoder.following(), Following::Next(frame_time(6)));
    decoder.frame_at(frame_time(29)).unwrap();
    assert_eq!(decoder.following(), Following::End);
    decoder.seek(frame_time(0)).unwrap();
    decoder.next_frame().unwrap();
    assert_eq!(decoder.following(), Following::Unknown);
}

#[test]
fn an_untagged_video_is_sdr_bt709() {
    let mut decoder = VideoDecoder::open(&sample(), Acceleration::Software).unwrap();
    let frame = decoder.frame_at(dusk_core::MediaTime(0)).unwrap().unwrap();
    let picture = frame.picture;
    assert_eq!(picture.primaries, dusk_core::color::Primaries::Bt709);
    assert_eq!(picture.transfer, dusk_core::color::Transfer::Bt1886);
    assert_eq!(picture.peak_nits, 0);
}

#[test]
fn frames_normalize_to_16_bit_yuv_at_the_size_asked_for() {
    let mut decoder = decoder();
    let mut times = Vec::new();
    while let Some(frame) = decoder.next_normalized((160, 120)).unwrap() {
        let picture = &frame.picture;
        assert_eq!((picture.width, picture.height), (160, 120));
        assert!(picture.planes.iter().all(|plane| plane.len() == 160 * 120));
        times.push(frame.time);
    }
    assert_eq!(times, (0..30).map(frame_time).collect::<Vec<_>>());
}

#[test]
fn normalizing_keeps_the_values_shifted_up_to_16_bits() {
    // At the picture's own size luma is only shifted; chroma is resampled to every pixel,
    // which keeps it where the picture is flat around a chroma sample, but for the rounding
    // of swscale's fixed-point filter. Converting the range or matrix would move it by
    // thousands.
    let picture = decoder().frame_at(MediaTime(0)).unwrap().unwrap().picture;
    let normalized = decoder()
        .next_normalized((320, 240))
        .unwrap()
        .unwrap()
        .picture;
    let shifted: Vec<u16> = picture.luma.iter().map(|y| u16::from(*y) << 8).collect();
    assert!(normalized.planes[0] == shifted, "luma changed");
    let chroma = |x: usize, y: usize| {
        let pair = y * 320 + x * 2;
        (picture.chroma[pair], picture.chroma[pair + 1])
    };
    let mut flat = 0;
    // The filter reaches two samples away on each side.
    for cy in 2..118usize {
        for cx in 2..158usize {
            let here = chroma(cx, cy);
            if !(cy - 2..=cy + 2).all(|ny| (cx - 2..=cx + 2).all(|nx| chroma(nx, ny) == here)) {
                continue;
            }
            flat += 1;
            // The luma pixel on the chroma sample's column, in its upper row.
            let index = 2 * cy * 320 + 2 * cx;
            for (plane, value) in [(1, here.0), (2, here.1)] {
                let normalized = normalized.planes[plane][index];
                let shifted = u16::from(value) << 8;
                assert!(
                    normalized.abs_diff(shifted) <= 16,
                    "plane {plane} at {cx},{cy}: {normalized} for {shifted}"
                );
            }
        }
    }
    assert!(flat > 160 * 120 / 4, "only {flat} flat chroma samples");
}

#[test]
fn normalized_frames_carry_the_color_tags() {
    let picture = decoder()
        .next_normalized((320, 240))
        .unwrap()
        .unwrap()
        .picture;
    assert_eq!(
        (picture.matrix, picture.range, picture.bits),
        (ColorMatrix::Bt601, ColorRange::Limited, 8)
    );
    assert_eq!(
        (picture.primaries, picture.transfer, picture.peak_nits),
        (Primaries::Bt709, Transfer::Bt1886, 0)
    );
}

#[test]
fn a_10_bit_video_normalizes_with_its_10_bits() {
    let mut decoder = VideoDecoder::open(&sample_10_bit(), Acceleration::Software).unwrap();
    let picture = decoder
        .next_normalized((320, 240))
        .unwrap()
        .unwrap()
        .picture;
    assert_eq!(
        (picture.width, picture.height, picture.bits),
        (320, 240, 10)
    );
    assert!(picture.planes[0].iter().all(|y| y % 64 == 0));
    assert!(picture.planes[0].iter().any(|y| (y >> 6) % 4 != 0));
}

#[test]
fn a_decoder_can_be_given_its_thread_count() {
    let mut decoder = VideoDecoder::open_with_threads(&sample(), 1).unwrap();
    let frames = std::iter::from_fn(|| decoder.next_normalized((32, 24)).unwrap()).count();
    assert_eq!(frames, 30);
}
