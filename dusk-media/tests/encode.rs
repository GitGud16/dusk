//! Writing MP4 files: H.264 from the first encoder in the order that opens (a hardware one
//! where there is one, OpenH264 on CI runners) and AAC audio.

use std::path::{Path, PathBuf};

use dusk_core::{ColorMatrix, ColorRange, MediaTime, Picture, PictureLayout};
use dusk_media::{
    Acceleration, AudioDecoder, AudioSettings, MediaError, Mp4Writer, StreamDetail, StreamKind,
    VideoDecoder, VideoSettings, probe,
};

/// A fresh output path in the target directory, so tests never write next to the sources.
fn output(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("encode");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path
}

/// A limited-range BT.709 NV12 picture: a gradient that moves with `n`.
fn gradient(width: u32, height: u32, n: u32) -> Picture {
    let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
    Picture {
        width,
        height,
        layout: PictureLayout::Nv12,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        primaries: dusk_core::color::Primaries::Bt709,
        transfer: dusk_core::color::Transfer::Bt1886,
        peak_nits: 0,
        luma: (0..height)
            .flat_map(|y| (0..width).map(move |x| (32 + (x + y + 2 * n) % 192) as u8))
            .collect(),
        chroma: (0..chroma_height)
            .flat_map(|y| {
                (0..chroma_width).flat_map(move |x| [(96 + x % 64) as u8, (96 + y % 64) as u8])
            })
            .collect(),
    }
}

/// `frames` frames of a 440 Hz tone, interleaved stereo at 48 kHz.
fn tone(start: usize, frames: usize) -> Vec<f32> {
    (start..start + frames)
        .flat_map(|n| {
            let sample = (std::f32::consts::TAU * 440.0 * n as f32 / 48_000.0).sin() * 0.5;
            [sample, sample]
        })
        .collect()
}

fn settings(width: u32, height: u32) -> VideoSettings {
    VideoSettings {
        width,
        height,
        frame_rate: (30, 1),
    }
}

/// Writes one second at 30 fps, with sound when `sound` is set.
fn write_second(path: &Path, width: u32, height: u32, sound: bool) -> String {
    let audio = sound.then_some(AudioSettings { rate: 48_000 });
    let mut writer = Mp4Writer::create(path, settings(width, height), audio).unwrap();
    let (width, height) = writer.size();
    for n in 0..30 {
        writer.write_video(&gradient(width, height, n)).unwrap();
        if sound {
            writer.write_audio(&tone(n as usize * 1600, 1600)).unwrap();
        }
    }
    let encoder = writer.encoder().to_owned();
    writer.finish().unwrap();
    encoder
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let squared: f64 = a
        .iter()
        .zip(b)
        .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
        .sum();
    10.0 * (255.0f64.powi(2) / (squared / a.len() as f64)).log10()
}

#[test]
fn a_second_of_video_and_sound_becomes_a_playable_mp4() {
    let path = output("second.mp4");
    let encoder = write_second(&path, 320, 240, true);
    assert!(
        ["h264_nvenc", "h264_qsv", "h264_amf", "libopenh264"].contains(&encoder.as_str()),
        "{encoder}"
    );

    let info = probe(&path).unwrap();
    assert!(info.format.contains("mp4"), "{}", info.format);
    let duration = info.duration_us.unwrap();
    assert!((950_000..=1_100_000).contains(&duration), "{duration} µs");
    let video = info
        .streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Video)
        .unwrap();
    assert_eq!(video.codec, "h264");
    assert!(matches!(
        video.detail,
        StreamDetail::Video {
            width: 320,
            height: 240,
            ..
        }
    ));
    let audio = info
        .streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Audio)
        .unwrap();
    assert_eq!(audio.codec, "aac");

    // The pictures come back close to what went in.
    let mut decoder = VideoDecoder::open(&path, Acceleration::Software).unwrap();
    let frame = decoder.frame_at(MediaTime(500_000)).unwrap().unwrap();
    let expected = gradient(320, 240, 15);
    let luma = psnr(&frame.picture.luma, &expected.luma);
    assert!(luma > 30.0, "luma PSNR {luma:.1} dB with {encoder}");
    assert_eq!(frame.picture.range, ColorRange::Limited);
    assert_eq!(frame.picture.matrix, ColorMatrix::Bt709);

    // And so does the sound.
    let mut sound = AudioDecoder::open(&path, 48_000, 2).unwrap();
    sound.seek(MediaTime(250_000)).unwrap();
    let mut samples = vec![0.0; 2 * 4800];
    assert_eq!(sound.read(&mut samples).unwrap(), 4800);
    let peak = samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
    assert!((0.4..0.6).contains(&peak), "peak {peak}");
}

#[test]
fn video_without_sound_has_one_stream() {
    let path = output("silent.mp4");
    write_second(&path, 160, 96, false);
    let info = probe(&path).unwrap();
    assert_eq!(info.streams.len(), 1);
    assert_eq!(info.streams[0].kind, StreamKind::Video);
}

#[test]
fn odd_sizes_are_rounded_down_to_what_encoders_take() {
    let path = output("odd.mp4");
    let writer = Mp4Writer::create(&path, settings(321, 241), None).unwrap();
    assert_eq!(writer.size(), (320, 240));
}

#[test]
fn sizes_beyond_the_encoders_limits_are_scaled_down_keeping_the_shape() {
    let path = output("large.mp4");
    let writer = Mp4Writer::create(&path, settings(8192, 2048), None).unwrap();
    let (width, height) = writer.size();
    assert!(width <= 4096 && height <= 4096, "{width}x{height}");
    assert_eq!((width, height), (4096, 1024));
}

#[test]
fn a_folder_that_does_not_exist_is_reported() {
    let path = output("missing").join("nowhere").join("out.mp4");
    assert!(matches!(
        Mp4Writer::create(&path, settings(64, 64), None),
        Err(MediaError::Create { .. })
    ));
}
