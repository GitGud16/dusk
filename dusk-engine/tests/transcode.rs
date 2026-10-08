//! dusq's CPU transcode path, which needs no graphics adapter: compressing a file and taking
//! its sound out, from the samples and from files these tests write.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use dusk_core::{ColorMatrix, ColorRange, MediaTime, Orientation, Picture, SdrPicture, yuv_to_rgb};
use dusk_engine::{
    EngineError, ExportFormat, ExportSettings, Progress, TranscodeSettings, extract_audio,
    transcode, transcode_to_size,
};
use dusk_media::{
    Acceleration, AudioCodec, AudioFormat, Container, MediaError, Quality, StreamDetail,
    StreamKind, Timing, VideoCodec, VideoDecoder, VideoSettings, Writer, probe,
};

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testdata")
        .join(name)
}

/// A fresh output path in the target directory.
fn output(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("transcode");
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

/// H.264 in MP4 from OpenH264, which every machine has, at the Small preset.
fn small() -> TranscodeSettings {
    TranscodeSettings {
        encoder: Some("libopenh264"),
        export: ExportSettings {
            quality: Quality::Level(40),
            ..ExportSettings::default()
        },
        ..TranscodeSettings::default()
    }
}

/// [`small`] with the short side at `side`.
fn sided(side: u32) -> TranscodeSettings {
    let mut settings = small();
    settings.export.short_side = Some(side);
    settings
}

/// Transcodes `input` with `settings`, keeping every progress report.
fn run(input: &Path, output: &Path, settings: &TranscodeSettings) -> Vec<Progress> {
    let mut reports = Vec::new();
    let done = transcode(
        input,
        output,
        settings,
        &AtomicBool::new(false),
        &mut |progress| reports.push(progress),
    )
    .unwrap()
    .expect("not cancelled");
    assert_eq!(done.path, output);
    assert!(!part(output).exists(), "the part file stayed");
    reports
}

fn video_of(path: &Path) -> StreamDetail {
    probe(path)
        .unwrap()
        .streams
        .into_iter()
        .find(|stream| stream.kind == StreamKind::Video)
        .expect("a video stream")
        .detail
}

fn size_of(path: &Path) -> (u32, u32) {
    match video_of(path) {
        StreamDetail::Video { width, height, .. } => (width, height),
        _ => unreachable!(),
    }
}

/// When each frame of `path` starts.
fn frame_times(path: &Path) -> Vec<i64> {
    let mut decoder = VideoDecoder::open(path, Acceleration::Software).unwrap();
    std::iter::from_fn(|| decoder.next_frame().unwrap())
        .map(|frame| frame.time.0)
        .collect()
}

#[test]
fn a_clip_is_compressed_with_its_picture_and_sound() {
    let path = output("compressed.mp4");
    let reports = run(&testdata("sample-h264-aac.mp4"), &path, &small());
    assert_eq!(size_of(&path), (320, 240));
    let info = probe(&path).unwrap();
    let audio = info
        .streams
        .iter()
        .find(|stream| stream.kind == StreamKind::Audio)
        .expect("sound");
    assert_eq!(audio.codec, "aac");
    assert_eq!(frame_times(&path).len(), 30);
    let duration = info.duration_us.unwrap();
    assert!((950_000..=1_100_000).contains(&duration), "{duration} µs");
    // Progress counts up to the whole length.
    let last = reports.last().expect("progress");
    assert_eq!(last.done, last.total);
    assert!(reports.windows(2).all(|pair| pair[0].done <= pair[1].done));
}

#[test]
fn a_turned_phone_clip_comes_out_upright() {
    let path = output("upright.mp4");
    run(&testdata("sample-rotated.mp4"), &path, &small());
    assert_eq!(size_of(&path), (240, 320));
    match video_of(&path) {
        StreamDetail::Video { orientation, .. } => assert_eq!(orientation, Orientation::UPRIGHT),
        _ => unreachable!(),
    }
}

#[test]
fn a_size_preset_sets_the_short_side() {
    let path = output("smaller.mp4");
    run(&testdata("sample-h264-aac.mp4"), &path, &sided(120));
    assert_eq!(size_of(&path), (160, 120));
}

#[test]
fn a_size_preset_never_enlarges() {
    let path = output("not-larger.mp4");
    run(&testdata("sample-h264-aac.mp4"), &path, &sided(1080));
    assert_eq!(size_of(&path), (320, 240));
}

/// Writes a clip of gray frames at `times`, without sound, to `path`.
fn clip_at(path: &Path, times: &[i64]) {
    let video = VideoSettings {
        encoder: Some("libopenh264"),
        timing: Timing::Source,
        ..VideoSettings::h264(64, 48, (30, 1))
    };
    let mut writer = Writer::create(path, Container::Mp4, video, None).unwrap();
    for (n, time) in times.iter().enumerate() {
        let gray = 40 + 20 * n as u8;
        let picture = SdrPicture {
            width: 64,
            height: 48,
            planes: [0; 3].map(|_| vec![gray; 64 * 48]),
        };
        writer.write_sdr(&picture, MediaTime(*time)).unwrap();
    }
    writer.finish().unwrap();
}

#[test]
fn a_variable_frame_rate_passes_through() {
    let times = [0, 40_000, 70_000, 140_000, 150_000, 200_000];
    let source = output("variable-source.mp4");
    clip_at(&source, &times);
    let path = output("variable.mp4");
    run(&source, &path, &small());
    let written = frame_times(&path);
    assert_eq!(written.len(), times.len());
    for (written, time) in written.iter().zip(times) {
        assert!((written - time).abs() <= 100, "{written} for {time}");
    }
}

#[test]
fn a_frame_rate_can_be_asked_for() {
    let path = output("ten-a-second.mp4");
    let settings = TranscodeSettings {
        fps: Some((10, 1)),
        ..small()
    };
    run(&testdata("sample-h264-aac.mp4"), &path, &settings);
    let written = frame_times(&path);
    assert_eq!(written.len(), 10);
    for (n, written) in written.iter().enumerate() {
        assert!(
            (written - n as i64 * 100_000).abs() <= 100,
            "{written} for frame {n}"
        );
    }
}

#[test]
fn hevc_in_mkv_comes_from_its_own_encoder() {
    let path = output("compressed.mkv");
    let mut settings = TranscodeSettings {
        encoder: Some("libkvazaar"),
        ..small()
    };
    settings.export.format = ExportFormat::Video {
        container: Container::Mkv,
        codec: VideoCodec::Hevc,
        audio: AudioCodec::Aac,
    };
    let done = transcode(
        &testdata("sample-h264-aac.mp4"),
        &path,
        &settings,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap()
    .unwrap();
    assert_eq!(done.encoder, "libkvazaar");
    let info = probe(&path).unwrap();
    let codecs: Vec<&str> = info.streams.iter().map(|s| s.codec.as_str()).collect();
    assert_eq!(codecs, ["hevc", "aac"]);
}

#[test]
fn a_cancelled_transcode_leaves_nothing_behind() {
    let path = output("cancelled.mp4");
    let done = transcode(
        &testdata("sample-h264-aac.mp4"),
        &path,
        &small(),
        &AtomicBool::new(true),
        &mut |_| {},
    )
    .unwrap();
    assert!(done.is_none());
    assert!(!path.exists() && !part(&path).exists());
}

#[test]
fn a_file_without_video_is_refused() {
    let path = output("no-video.mp4");
    let refused = transcode(
        &testdata("voice.m4a"),
        &path,
        &small(),
        &AtomicBool::new(false),
        &mut |_| {},
    );
    assert!(matches!(
        refused,
        Err(EngineError::Media(MediaError::NoVideo { .. }))
    ));
    assert!(!path.exists() && !part(&path).exists());
}

#[test]
fn the_sound_alone_is_taken_out() {
    let path = output("sound.mp3");
    let mut reports = Vec::new();
    let done = extract_audio(
        &testdata("sample-h264-aac.mp4"),
        &path,
        AudioFormat::Mp3,
        &AtomicBool::new(false),
        &mut |progress| reports.push(progress),
    )
    .unwrap()
    .unwrap();
    assert_eq!(done.encoder, "libmp3lame");
    let info = probe(&path).unwrap();
    assert_eq!(info.streams.len(), 1);
    assert_eq!(info.streams[0].kind, StreamKind::Audio);
    assert_eq!(info.streams[0].codec, "mp3");
    let duration = info.duration_us.unwrap();
    assert!((950_000..=1_100_000).contains(&duration), "{duration} µs");
    let last = reports.last().expect("progress");
    assert_eq!(last.done, last.total);
}

#[test]
fn every_sound_format_can_be_taken_out() {
    for format in AudioFormat::ALL {
        let path = output(&format!("every-sound.{}", format.extension()));
        extract_audio(
            &testdata("voice.opus"),
            &path,
            format,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap()
        .unwrap();
        let codec = &probe(&path).unwrap().streams[0].codec;
        let expected = match format.codec() {
            AudioCodec::Aac => "aac",
            AudioCodec::Opus => "opus",
            AudioCodec::Mp3 => "mp3",
            AudioCodec::Pcm => "pcm_s16le",
        };
        assert_eq!(codec, expected, "{format:?}");
    }
}

#[test]
fn a_file_without_sound_has_none_to_take_out() {
    let source = output("silent-source.mp4");
    clip_at(&source, &[0, 33_333]);
    let path = output("silent.mp3");
    let refused = extract_audio(
        &source,
        &path,
        AudioFormat::Mp3,
        &AtomicBool::new(false),
        &mut |_| {},
    );
    assert!(matches!(
        refused,
        Err(EngineError::Media(MediaError::NoAudio { .. }))
    ));
    assert!(!path.exists() && !part(&path).exists());
}

/// The picture of `path` at `time`.
fn picture_at(path: &Path, time: MediaTime) -> Picture {
    let mut decoder = VideoDecoder::open(path, Acceleration::Software).unwrap();
    decoder.frame_at(time).unwrap().unwrap().picture
}

/// Pixel `(x, y)` of `picture` as RGB from 0 to 255, by the picture's own tags, its chroma
/// from the nearest sample.
fn rgb(picture: &Picture, x: usize, y: usize) -> [f64; 3] {
    let width = picture.width as usize;
    let pair = (y / 2) * width + (x / 2) * 2;
    let yuv = [
        f64::from(picture.luma[y * width + x]),
        f64::from(picture.chroma[pair]),
        f64::from(picture.chroma[pair + 1]),
    ];
    yuv_to_rgb(picture.matrix, picture.range, 8)
        .map(|[a, b, c, d]| (a * yuv[0] + b * yuv[1] + c * yuv[2] + d).clamp(0.0, 1.0) * 255.0)
}

#[test]
fn the_colors_survive_the_trip() {
    // The sample is untagged SD video, so BT.601; what dusq writes is tagged BT.709. Where
    // the picture is flat, resampling changes nothing, so the written YUV must be the source's
    // color in BT.709 but for rounding and what encoding at a generous bitrate costs. Another
    // matrix or range is 5 to 25 off.
    let path = output("colors.mp4");
    let settings = TranscodeSettings {
        encoder: Some("libopenh264"),
        export: ExportSettings {
            quality: Quality::Bitrate(20_000_000),
            ..ExportSettings::default()
        },
        ..TranscodeSettings::default()
    };
    run(&testdata("sample-h264-aac.mp4"), &path, &settings);
    // The first frame, a keyframe: later ones carry the encoder's scattered artifacts on.
    let source = picture_at(&testdata("sample-h264-aac.mp4"), MediaTime(0));
    let written = picture_at(&path, MediaTime(0));
    assert_eq!(
        (written.matrix, written.range),
        (ColorMatrix::Bt709, ColorRange::Limited)
    );
    let chroma = |x: usize, y: usize| {
        let pair = y * 320 + x * 2;
        (source.chroma[pair], source.chroma[pair + 1])
    };
    let mut flat = 0;
    // Written chroma is filtered from source pixels up to about four away.
    for cy in 3..117usize {
        for cx in 3..157usize {
            let here = chroma(cx, cy);
            let around = |n: usize, reach: usize| n - reach..=n + reach;
            if !around(cy, 3).all(|ny| around(cx, 3).all(|nx| chroma(nx, ny) == here)) {
                continue;
            }
            let (x, y) = (2 * cx, 2 * cy);
            let luma = source.luma[y * 320 + x];
            let flat_luma =
                around(y, 6).all(|ny| around(x, 6).all(|nx| source.luma[ny * 320 + nx] == luma));
            if !flat_luma {
                continue;
            }
            flat += 1;
            // The source's color in limited-range BT.709 YUV, as dusq should write it.
            let [red, green, blue] = rgb(&source, x, y).map(|value| value / 255.0);
            let luma = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
            let expected = [
                16.0 + 219.0 * luma,
                128.0 + 224.0 * (blue - luma) / 1.8556,
                128.0 + 224.0 * (red - luma) / 1.5748,
            ];
            let pair = cy * 320 + 2 * cx;
            let actual = [
                written.luma[y * 320 + x],
                written.chroma[pair],
                written.chroma[pair + 1],
            ];
            for ((actual, expected), name) in actual.iter().zip(expected).zip(["Y", "U", "V"]) {
                assert!(
                    (f64::from(*actual) - expected).abs() <= 2.0,
                    "{name} at {x},{y}: {actual} for {expected:.1}"
                );
            }
        }
    }
    assert!(flat > 500, "only {flat} flat pixels");
}

/// Writes `frames` frames of `width` by `height` at 30 a second without sound to `path`: a
/// gradient, with noise on it when `noisy`, which encoders cannot squeeze much.
fn made_up(path: &Path, (width, height): (u32, u32), frames: u32, noisy: bool) {
    let video = VideoSettings {
        encoder: Some("libopenh264"),
        quality: Quality::Level(95),
        ..VideoSettings::h264(width, height, (30, 1))
    };
    let mut writer = Writer::create(path, Container::Mp4, video, None).unwrap();
    let mut state = 0x9e37_79b9_u32;
    for n in 0..frames {
        let count = (width * height) as usize;
        let planes = [0u32, 1, 2].map(|plane| {
            (0..count)
                .map(|i| {
                    let x = (i as u32 % width + n * 4 + plane * 40) % 256;
                    let noise = if noisy {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        state >> 26
                    } else {
                        0
                    };
                    (x / 2 + 40 + noise) as u8
                })
                .collect()
        });
        let picture = SdrPicture {
            width,
            height,
            planes,
        };
        writer
            .write_sdr(&picture, MediaTime(i64::from(n) * 33_333))
            .unwrap();
    }
    writer.finish().unwrap();
}

/// Compresses `input` to about `target` bytes, keeping every progress report.
fn to_size(
    input: &Path,
    output: &Path,
    target: u64,
) -> (
    Result<Option<dusk_engine::Transcoded>, EngineError>,
    Vec<Progress>,
) {
    let mut reports = Vec::new();
    let done = transcode_to_size(
        input,
        output,
        target,
        &small(),
        &AtomicBool::new(false),
        &mut |progress| reports.push(progress),
    );
    (done, reports)
}

#[test]
fn a_size_target_picks_the_picture_size_from_the_ladder() {
    let source = output("hd-source.mp4");
    made_up(&source, (1280, 720), 60, false);
    // 300 KB over 2 s leaves 1.16 Mbit/s for the video: 480p.
    let path = output("to-size.mp4");
    let (done, _) = to_size(&source, &path, 300_000);
    let done = done.unwrap().unwrap();
    assert_eq!(size_of(&path), (854, 480));
    assert_eq!(done.size, (854, 480));
    assert!(!part(&path).exists());
}

#[test]
fn a_size_below_the_floor_is_refused_with_the_smallest() {
    let path = output("too-small.mp4");
    let (done, _) = to_size(&testdata("sample-h264-aac.mp4"), &path, 10_000);
    // 248 kbit/s for a second, with the container's share: 31,959 bytes.
    assert!(
        matches!(done, Err(EngineError::TooSmall { smallest: 31_959 })),
        "{done:?}"
    );
    assert!(!path.exists() && !part(&path).exists());
}

#[test]
fn a_file_too_far_over_is_made_again_smaller() {
    // Half a second: the container's own share of so small a file is more than the 3% the
    // plan leaves it, so the first pass comes out too far over and a second runs.
    let source = output("short-source.mp4");
    made_up(&source, (320, 240), 15, true);
    let path = output("short-to-size.mp4");
    let target = 15_000;
    let (done, reports) = to_size(&source, &path, target);
    let done = done.unwrap().unwrap();
    assert!(
        reports.iter().any(|report| report.pass == 2),
        "one pass only"
    );
    assert!(reports.iter().all(|report| report.pass <= 2));
    assert!(done.bytes > 0 && !part(&path).exists());
}
