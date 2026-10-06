//! The GPU and CPU paths held against each other (docs/ARCHITECTURE.md, "Compress tool
//! paths"). They cannot be bit-identical, so the pre-encode YUV frames of the same source at the
//! same output size must come within a tolerance: PSNR of at least 45 dB unscaled and 40 dB
//! scaled, every plane. The sample set covers untagged HD video, a turned phone clip, a Display
//! P3 photo, HDR and a 4K-to-480p downscale.

use std::path::{Path, PathBuf};

use dusk_core::color::SdrConverter;
use dusk_core::time::frame_at;
use dusk_core::{MediaKind, MediaTime, Picture, SdrPicture};
use dusk_media::{
    Acceleration, Container, Quality, VideoDecoder, VideoSettings, Writer, decode_still,
    decode_still_normalized, sdr_to_nv12,
};
use dusk_render::{Compositor, Gpu, ToYuv};

use crate::compress::compress_project;
use crate::info::{media_info, still_size};
use crate::placement::placement_at;
use crate::transcode::upright;

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testdata")
        .join(name)
}

/// The GPU path's pre-encode frame of the file at `path` at `time`, drawn at `size`: its
/// one-clip project through the compositor and into the encoder's YUV, as an export does.
fn gpu_frame(gpu: &Gpu, path: &Path, time: MediaTime, size: (u32, u32)) -> Picture {
    let info = media_info(path).unwrap();
    let project = compress_project(path.to_path_buf(), info.clone()).unwrap();
    let sequence = project.sequence();
    let frame = frame_at(time, sequence.frame_rate());
    let picture = if info.kind == MediaKind::Still {
        decode_still(path, still_size(&info, sequence.resolution())).unwrap()
    } else {
        let clip = sequence.visible_video_at(frame).unwrap();
        let source_time = clip.source_time_at(frame, sequence.frame_rate());
        let mut decoder = VideoDecoder::open(path, Acceleration::Software).unwrap();
        decoder.frame_at(source_time).unwrap().unwrap().picture
    };
    let placement = placement_at(&project, frame);
    let texture = Compositor::new(gpu)
        .render_placed(&picture, &placement, size)
        .unwrap();
    ToYuv::new(gpu).convert(&texture).unwrap()
}

/// The CPU path's pre-encode frame of the file at `path` at `time`, at `size` (upright):
/// normalized, through color steps 2 to 4 and turned upright, then into the encoder's YUV, as
/// dusq does.
fn cpu_frame(path: &Path, time: MediaTime, size: (u32, u32)) -> Picture {
    let info = media_info(path).unwrap();
    // Frames are normalized as stored, before they are turned.
    let stored = info.orientation.apply_to_size(size);
    let picture = if info.kind == MediaKind::Still {
        decode_still_normalized(path, stored).unwrap()
    } else {
        let mut decoder = VideoDecoder::open_with_threads(path, 1).unwrap();
        let mut shown = None;
        while let Some(frame) = decoder.next_normalized(stored).unwrap() {
            if frame.time > time {
                break;
            }
            shown = Some(frame.picture);
        }
        shown.unwrap()
    };
    let converter = SdrConverter::new(
        picture.primaries,
        picture.transfer,
        f64::from(picture.peak_nits),
    );
    let sdr = upright(&picture, converter.as_ref(), info.orientation, 1);
    sdr_to_nv12(&sdr).unwrap()
}

/// The Y, U and V planes of an NV12 picture.
fn planes(picture: &Picture) -> [Vec<u8>; 3] {
    let (pairs, _) = picture.chroma.as_chunks::<2>();
    [
        picture.luma.clone(),
        pairs.iter().map(|[u, _]| *u).collect(),
        pairs.iter().map(|[_, v]| *v).collect(),
    ]
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let squared: f64 = a
        .iter()
        .zip(b)
        .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
        .sum();
    if squared == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64.powi(2) / (squared / a.len() as f64)).log10()
}

/// Holds the two paths' frames of `name` to `floor` dB, every plane.
fn assert_close(name: &str, gpu: &Picture, cpu: &Picture, floor: f64) {
    assert_eq!((gpu.width, gpu.height), (cpu.width, cpu.height), "{name}");
    for ((gpu, cpu), plane) in planes(gpu)
        .iter()
        .zip(planes(cpu).iter())
        .zip(["Y", "U", "V"])
    {
        let psnr = psnr(gpu, cpu);
        assert!(psnr >= floor, "{name}, {plane}: {psnr:.1} dB");
    }
}

fn gpu() -> Gpu {
    Gpu::new().expect("a graphics adapter: hardware, or WARP on CI")
}

#[test]
fn untagged_hd_video_comes_out_the_same_both_ways() {
    let path = testdata("sample-hd-untagged.mp4");
    let time = MediaTime(200_000);
    let size = (1280, 720);
    assert_close(
        "untagged HD",
        &gpu_frame(&gpu(), &path, time, size),
        &cpu_frame(&path, time, size),
        45.0,
    );
}

#[test]
fn a_turned_phone_clip_comes_out_the_same_both_ways() {
    let path = testdata("sample-rotated.mp4");
    let time = MediaTime(200_000);
    let size = (240, 320);
    assert_close(
        "turned phone clip",
        &gpu_frame(&gpu(), &path, time, size),
        &cpu_frame(&path, time, size),
        45.0,
    );
}

#[test]
fn a_display_p3_photo_comes_out_the_same_both_ways() {
    let path = testdata("photo-p3.jpg");
    let size = (320, 240);
    assert_close(
        "Display P3 photo",
        &gpu_frame(&gpu(), &path, MediaTime(0), size),
        &cpu_frame(&path, MediaTime(0), size),
        45.0,
    );
}

#[test]
fn hdr_video_comes_out_the_same_both_ways() {
    let path = testdata("sample-hlg.mp4");
    let time = MediaTime(200_000);
    let size = (320, 240);
    assert_close(
        "HLG",
        &gpu_frame(&gpu(), &path, time, size),
        &cpu_frame(&path, time, size),
        45.0,
    );
}

/// Writes two frames of 4K to `path`: bars, rings and slopes, detail a 480p picture can still
/// show, so the two downscalers are compared rather than how each folds away detail too fine
/// for 480p.
fn four_k(path: &Path) {
    let (width, height) = (3840u32, 2160u32);
    let video = VideoSettings {
        encoder: Some("libopenh264"),
        quality: Quality::Level(95),
        ..VideoSettings::h264(width, height, (30, 1))
    };
    let mut writer = Writer::create(path, Container::Mp4, video, None).unwrap();
    for n in 0..2 {
        let pixel = |x: u32, y: u32, plane: u32| -> u8 {
            let bars = (x * 7 / width + plane) % 3 * 60;
            let (dx, dy) = (f64::from(x) - 1920.0, f64::from(y) - 1080.0);
            let ring = ((dx * dx + dy * dy).sqrt() / 24.0).sin() * 40.0;
            let slope = f64::from((x + y + 40 * n) % 512) / 8.0;
            (f64::from(bars) + 60.0 + ring + slope).clamp(0.0, 255.0) as u8
        };
        let planes = [0, 1, 2].map(|plane| {
            (0..height)
                .flat_map(|y| (0..width).map(move |x| pixel(x, y, plane)))
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

#[test]
fn a_4k_video_downscaled_to_480p_comes_out_the_same_both_ways() {
    // Unit tests have no target folder of their own to write in.
    let dir = std::env::temp_dir().join(format!("dusk-equivalence-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("4k.mp4");
    four_k(&path);
    let size = (854, 480);
    assert_close(
        "4K to 480p",
        &gpu_frame(&gpu(), &path, MediaTime(0), size),
        &cpu_frame(&path, MediaTime(0), size),
        40.0,
    );
}
