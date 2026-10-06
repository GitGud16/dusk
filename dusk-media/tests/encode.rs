//! Writing export files: video in every container with each codec it holds, from the first
//! encoder of the codec that opens (a hardware one where there is one) or from the software
//! encoder CI runners have, and sound alone in each sound format.

use std::path::{Path, PathBuf};

use dusk_core::{ColorMatrix, ColorRange, MediaTime, Picture, PictureLayout, SdrPicture};
use dusk_media::{
    Acceleration, AudioCodec, AudioDecoder, AudioFormat, AudioSettings, Container, MediaError,
    Quality, StreamDetail, StreamKind, Timing, VideoCodec, VideoDecoder, VideoSettings, Writer,
    encoders_of, probe,
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

/// The same picture with noise on it, which encoders cannot squeeze much.
fn noisy(width: u32, height: u32, n: u32) -> Picture {
    let mut picture = gradient(width, height, n);
    let mut state = 0x9e37_79b9_u32.wrapping_mul(n + 1);
    for sample in &mut picture.luma {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *sample = sample.saturating_add((state >> 27) as u8).min(235);
    }
    picture
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

/// H.264 from the first encoder that opens, at the High preset.
fn settings(width: u32, height: u32) -> VideoSettings {
    VideoSettings::h264(width, height, (30, 1))
}

/// Writes `frames` frames at 30 fps, with a second of sound per 30 frames when `audio` is
/// set; returns the encoder used.
fn write(
    path: &Path,
    container: Container,
    video: VideoSettings,
    audio: Option<AudioSettings>,
    frames: u32,
    picture: fn(u32, u32, u32) -> Picture,
) -> String {
    let mut writer = Writer::create(path, container, video, audio).unwrap();
    let (width, height) = writer.size();
    for n in 0..frames {
        writer.write_video(&picture(width, height, n)).unwrap();
        if audio.is_some() {
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

fn codec_of(path: &Path, kind: StreamKind) -> String {
    probe(path)
        .unwrap()
        .streams
        .iter()
        .find(|stream| stream.kind == kind)
        .map(|stream| stream.codec.clone())
        .unwrap_or_default()
}

/// The software encoder of `codec`, which every machine has.
fn software(codec: VideoCodec) -> &'static str {
    encoders_of(codec)
        .find(|encoder| !encoder.hardware)
        .map(|encoder| encoder.name)
        .unwrap()
}

#[test]
fn a_second_of_video_and_sound_becomes_a_playable_mp4() {
    let path = output("second.mp4");
    let audio = Some(AudioSettings::of(AudioCodec::Aac, 48_000));
    let encoder = write(
        &path,
        Container::Mp4,
        settings(320, 240),
        audio,
        30,
        gradient,
    );
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
    assert_eq!(codec_of(&path, StreamKind::Audio), "aac");

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
    write(&path, Container::Mp4, settings(160, 96), None, 30, gradient);
    let info = probe(&path).unwrap();
    assert_eq!(info.streams.len(), 1);
    assert_eq!(info.streams[0].kind, StreamKind::Video);
}

#[test]
fn every_container_holds_each_of_its_codecs() {
    for container in Container::ALL {
        for &codec in container.video_codecs() {
            let expected = match codec {
                VideoCodec::H264 => "h264",
                VideoCodec::Hevc => "hevc",
                VideoCodec::Av1 => "av1",
                VideoCodec::Vp9 => "vp9",
            };
            let name = format!("holds-{expected}.{}", container.extension());
            let path = output(&name);
            let audio = Some(AudioSettings::of(container.audio_codecs()[0], 48_000));
            let video = VideoSettings {
                codec,
                encoder: Some(software(codec)),
                ..settings(160, 96)
            };
            let encoder = write(&path, container, video, audio, 10, gradient);
            assert_eq!(encoder, software(codec));
            assert_eq!(codec_of(&path, StreamKind::Video), expected, "{name}");
            let sound = match container.audio_codecs()[0] {
                AudioCodec::Aac => "aac",
                _ => "opus",
            };
            assert_eq!(codec_of(&path, StreamKind::Audio), sound, "{name}");
        }
    }
}

#[test]
fn mkv_holds_opus_sound_too() {
    let path = output("opus.mkv");
    let audio = Some(AudioSettings::of(AudioCodec::Opus, 48_000));
    let video = VideoSettings {
        encoder: Some("libopenh264"),
        ..settings(160, 96)
    };
    write(&path, Container::Mkv, video, audio, 30, gradient);
    assert_eq!(codec_of(&path, StreamKind::Audio), "opus");
    let mut sound = AudioDecoder::open(&path, 48_000, 2).unwrap();
    sound.seek(MediaTime(250_000)).unwrap();
    let mut samples = vec![0.0; 2 * 4800];
    assert_eq!(sound.read(&mut samples).unwrap(), 4800);
    let peak = samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
    assert!((0.4..0.6).contains(&peak), "peak {peak}");
}

#[test]
fn hevc_in_mp4_is_tagged_the_way_apple_players_need() {
    let path = output("tagged.mp4");
    let video = VideoSettings {
        codec: VideoCodec::Hevc,
        encoder: Some("libkvazaar"),
        ..settings(160, 96)
    };
    write(&path, Container::Mp4, video, None, 10, gradient);
    let bytes = std::fs::read(&path).unwrap();
    let has = |tag: &[u8]| bytes.windows(4).any(|window| window == tag);
    assert!(has(b"hvc1") && !has(b"hev1"));
}

#[test]
fn a_file_format_that_cannot_hold_the_codec_is_refused() {
    let path = output("refused.webm");
    let refused = Writer::create(&path, Container::WebM, settings(160, 96), None);
    assert!(matches!(
        refused,
        Err(MediaError::Unsupported {
            format: "WebM",
            codec: "H.264"
        })
    ));
    let aac = Some(AudioSettings::of(AudioCodec::Aac, 48_000));
    let vp9 = VideoSettings {
        codec: VideoCodec::Vp9,
        ..settings(160, 96)
    };
    assert!(matches!(
        Writer::create(&path, Container::WebM, vp9, aac),
        Err(MediaError::Unsupported { .. })
    ));
    let wav = AudioSettings::of(AudioCodec::Mp3, 48_000);
    assert!(matches!(
        Writer::sound(&output("refused.wav"), AudioFormat::Wav, wav),
        Err(MediaError::Unsupported { .. })
    ));
}

#[test]
fn lower_quality_writes_smaller_files() {
    let size_at = |quality: Quality, name: &str| {
        let path = output(name);
        let video = VideoSettings {
            codec: VideoCodec::Hevc,
            encoder: Some("libkvazaar"),
            quality,
            ..settings(320, 240)
        };
        write(&path, Container::Mkv, video, None, 30, noisy);
        std::fs::metadata(&path).unwrap().len()
    };
    let high = size_at(Quality::HIGH, "high.mkv");
    let small = size_at(Quality::SMALL, "small.mkv");
    assert!(small < high, "{small} bytes against {high}");
}

#[test]
fn a_target_bitrate_lands_near_it() {
    let path = output("target.mp4");
    let video = VideoSettings {
        encoder: Some("libopenh264"),
        quality: Quality::Bitrate(1_000_000),
        ..settings(640, 360)
    };
    write(&path, Container::Mp4, video, None, 60, noisy);
    let bytes = std::fs::metadata(&path).unwrap().len() as f64;
    // Two seconds at 1 Mbit/s are 250 kB.
    assert!(
        (150_000.0..=350_000.0).contains(&bytes),
        "{bytes} bytes for 2 s at 1 Mbit/s"
    );
}

#[test]
fn sound_alone_is_written_in_each_sound_format() {
    for format in AudioFormat::ALL {
        let path = output(&format!("sound.{}", format.extension()));
        let mut writer =
            Writer::sound(&path, format, AudioSettings::of(format.codec(), 48_000)).unwrap();
        assert_eq!(writer.size(), (0, 0));
        for second in 0..2 {
            writer.write_audio(&tone(second * 48_000, 48_000)).unwrap();
        }
        writer.finish().unwrap();
        let info = probe(&path).unwrap();
        assert_eq!(info.streams.len(), 1, "{format:?}");
        let codec = &info.streams[0].codec;
        let expected = match format {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::M4a => "aac",
            AudioFormat::Opus => "opus",
            AudioFormat::Wav => "pcm_s16le",
        };
        assert_eq!(codec, expected);
        let duration = info.duration_us.unwrap();
        assert!(
            (1_950_000..=2_100_000).contains(&duration),
            "{format:?}: {duration} µs"
        );
        let mut sound = AudioDecoder::open(&path, 48_000, 2).unwrap();
        sound.seek(MediaTime(500_000)).unwrap();
        let mut samples = vec![0.0; 2 * 4800];
        assert_eq!(sound.read(&mut samples).unwrap(), 4800);
        let peak = samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!((0.4..0.6).contains(&peak), "{format:?}: peak {peak}");
    }
}

#[test]
fn odd_sizes_are_rounded_down_to_what_encoders_take() {
    let path = output("odd.mp4");
    let writer = Writer::create(&path, Container::Mp4, settings(321, 241), None).unwrap();
    assert_eq!(writer.size(), (320, 240));
}

#[test]
fn sizes_beyond_the_encoders_limits_are_scaled_down_keeping_the_shape() {
    let path = output("large.mp4");
    let writer = Writer::create(&path, Container::Mp4, settings(8192, 2048), None).unwrap();
    let (width, height) = writer.size();
    assert!(width <= 4096 && height <= 4096, "{width}x{height}");
    assert_eq!((width, height), (4096, 1024));
}

#[test]
fn a_folder_that_does_not_exist_is_reported() {
    let path = output("missing").join("nowhere").join("out.mp4");
    assert!(matches!(
        Writer::create(&path, Container::Mp4, settings(64, 64), None),
        Err(MediaError::Create { .. })
    ));
}

#[test]
fn every_software_encoder_is_available_and_the_list_keeps_the_order() {
    let available = dusk_media::available_encoders();
    let names: Vec<&str> = available.iter().map(|encoder| encoder.name).collect();
    for software in ["libopenh264", "libkvazaar", "libsvtav1", "libvpx-vp9"] {
        assert!(names.contains(&software), "{software} in {names:?}");
    }
    let order: Vec<&str> = dusk_media::ENCODERS
        .iter()
        .map(|encoder| encoder.name)
        .filter(|name| names.contains(name))
        .collect();
    assert_eq!(names, order);
    // Asked again, the same list comes back at once.
    let again = std::time::Instant::now();
    assert_eq!(dusk_media::available_encoders().len(), available.len());
    assert!(again.elapsed() < std::time::Duration::from_millis(5));
}

/// A `width` by `height` upright SDR picture of one color.
fn solid(width: u32, height: u32, rgb: [u8; 3]) -> SdrPicture {
    let count = (width * height) as usize;
    SdrPicture {
        width,
        height,
        planes: rgb.map(|value| vec![value; count]),
    }
}

/// Writes `times` as frames of `rgb` from `encoder` into a `container` file with `timing`.
fn write_sdr(
    path: &Path,
    container: Container,
    encoder: &'static str,
    timing: Timing,
    times: &[i64],
    rgb: [u8; 3],
) {
    let video = VideoSettings {
        encoder: Some(encoder),
        timing,
        ..settings(64, 48)
    };
    let mut writer = Writer::create(path, container, video, None).unwrap();
    for time in times {
        writer
            .write_sdr(&solid(64, 48, rgb), MediaTime(*time))
            .unwrap();
    }
    writer.finish().unwrap();
}

/// When each frame of `path` starts.
fn frame_times(path: &Path) -> Vec<i64> {
    let mut decoder = VideoDecoder::open(path, Acceleration::Software).unwrap();
    std::iter::from_fn(|| decoder.next_frame().unwrap())
        .map(|frame| frame.time.0)
        .collect()
}

#[test]
fn an_sdr_picture_is_written_in_the_encoders_own_yuv() {
    let path = output("sdr.mp4");
    let rgb = [200, 40, 90];
    write_sdr(
        &path,
        Container::Mp4,
        "libopenh264",
        Timing::Constant,
        &[0, 1, 2],
        rgb,
    );
    let mut decoder = VideoDecoder::open(&path, Acceleration::Software).unwrap();
    let picture = decoder.frame_at(MediaTime(0)).unwrap().unwrap().picture;
    assert_eq!(
        (picture.matrix, picture.range),
        (ColorMatrix::Bt709, ColorRange::Limited)
    );
    // The color in limited-range BT.709 YUV, as the encoder should have been given it.
    let [red, green, blue] = rgb.map(|value| f64::from(value) / 255.0);
    let luma = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
    let expected = [
        16.0 + 219.0 * luma,
        128.0 + 224.0 * (blue - luma) / 1.8556,
        128.0 + 224.0 * (red - luma) / 1.5748,
    ];
    let center = 24 * 64 + 32;
    let pair = 12 * 64 + 32;
    let written = [
        picture.luma[center],
        picture.chroma[pair],
        picture.chroma[pair + 1],
    ];
    // Within what encoding a flat picture costs; another matrix or range is 5 to 13 off.
    for (written, expected) in written.iter().zip(expected) {
        assert!(
            (f64::from(*written) - expected).abs() <= 2.0,
            "{written} for {expected:.1}"
        );
    }
}

#[test]
fn source_timing_keeps_each_frames_time() {
    // A phone's variable frame rate: gaps of 40, 30, 70 and 10 ms.
    let times = [0, 40_000, 70_000, 140_000, 150_000];
    for container in [Container::Mp4, Container::Mkv] {
        let path = output(&format!("source-timing.{}", container.extension()));
        write_sdr(
            &path,
            container,
            "libopenh264",
            Timing::Source,
            &times,
            [90; 3],
        );
        let written = frame_times(&path);
        assert_eq!(written.len(), times.len(), "{container:?}");
        for (written, time) in written.iter().zip(times) {
            // MKV counts milliseconds.
            assert!(
                (written - time).abs() <= 1_000,
                "{container:?}: {written} for {time}"
            );
        }
    }
}

#[test]
fn constant_timing_spaces_the_frames_evenly() {
    let path = output("constant-timing.mp4");
    write_sdr(
        &path,
        Container::Mp4,
        "libopenh264",
        Timing::Constant,
        &[0, 40_000, 70_000],
        [90; 3],
    );
    let written = frame_times(&path);
    assert_eq!(written.len(), 3);
    for (written, time) in written.iter().zip([0, 33_333, 66_667]) {
        assert!((written - time).abs() <= 1, "{written} for {time}");
    }
}

#[test]
fn a_picture_of_another_size_is_refused() {
    let path = output("wrong-size.mp4");
    let video = VideoSettings {
        encoder: Some("libopenh264"),
        ..settings(64, 48)
    };
    let mut writer = Writer::create(&path, Container::Mp4, video, None).unwrap();
    assert!(
        writer
            .write_sdr(&solid(32, 48, [0; 3]), MediaTime(0))
            .is_err()
    );
}
