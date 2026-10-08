//! The export pass: composited RGB frames to limited-range BT.709 4:2:0, read back. Needs a
//! graphics adapter; CI runners use WARP (see tests/gpu.rs).

use dusk_core::{ColorMatrix, ColorRange, Picture, PictureLayout};
use dusk_render::{Compositor, Gpu, ToYuv};

fn gpu() -> Gpu {
    Gpu::new().expect("a graphics adapter: hardware, or WARP on CI")
}

/// An 8-bit BT.709 limited-range picture whose planes come from `luma` and `chroma`.
fn picture(
    width: u32,
    height: u32,
    luma: impl Fn(u32, u32) -> u8,
    chroma: impl Fn(u32, u32) -> [u8; 2],
) -> Picture {
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
        siting: dusk_core::ChromaSiting::LEFT,
        luma: (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| luma(x, y))
            .collect(),
        chroma: (0..chroma_height)
            .flat_map(|y| (0..chroma_width).map(move |x| (x, y)))
            .flat_map(|(x, y)| chroma(x, y))
            .collect(),
    }
}

/// Draws `source` at its own size and converts it back to YUV.
fn round_trip(gpu: &Gpu, source: &Picture) -> Picture {
    let compositor = Compositor::new(gpu);
    let frame = compositor
        .render(source, (source.width, source.height))
        .unwrap();
    ToYuv::new(gpu).convert(&frame).unwrap()
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let squared: f64 = a
        .iter()
        .zip(b)
        .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
        .sum();
    let mse = squared / a.len() as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64.powi(2) / mse).log10()
    }
}

#[test]
fn the_result_is_tagged_limited_range_bt709_nv12() {
    let gpu = gpu();
    let gray = picture(64, 32, |_, _| 126, |_, _| [128, 128]);
    let yuv = round_trip(&gpu, &gray);
    assert_eq!((yuv.width, yuv.height), (64, 32));
    assert_eq!(yuv.layout, PictureLayout::Nv12);
    assert_eq!(
        (yuv.matrix, yuv.range),
        (ColorMatrix::Bt709, ColorRange::Limited)
    );
    assert_eq!(yuv.luma.len(), 64 * 32);
    assert_eq!(yuv.chroma.len(), 32 * 16 * 2);
}

#[test]
fn flat_colors_come_back_as_they_went_in() {
    let gpu = gpu();
    // Gray, and the textbook BT.709 red.
    for [y, u, v] in [
        [126, 128, 128],
        [63, 102, 240],
        [16, 128, 128],
        [235, 128, 128],
    ] {
        let flat = picture(32, 16, |_, _| y, |_, _| [u, v]);
        let yuv = round_trip(&gpu, &flat);
        assert!(
            yuv.luma.iter().all(|code| code.abs_diff(y) <= 1),
            "Y of {y}"
        );
        let (pairs, _) = yuv.chroma.as_chunks::<2>();
        assert!(
            pairs
                .iter()
                .all(|[cu, cv]| cu.abs_diff(u) <= 1 && cv.abs_diff(v) <= 1),
            "UV of {u},{v}: {:?}",
            &pairs[..4]
        );
    }
}

#[test]
fn smooth_pictures_survive_the_round_trip() {
    let gpu = gpu();
    let (width, height) = (128, 96);
    // Gradients that stay inside the legal range, so nothing clips in RGB.
    let source = picture(
        width,
        height,
        |x, y| (48 + x / 2 + y / 2) as u8,
        |x, y| [(108 + x / 4) as u8, (148 - y / 4) as u8],
    );
    let yuv = round_trip(&gpu, &source);
    let luma = psnr(&yuv.luma, &source.luma);
    let chroma = psnr(&yuv.chroma, &source.chroma);
    assert!(luma >= 45.0, "luma PSNR {luma:.1} dB");
    assert!(chroma >= 40.0, "chroma PSNR {chroma:.1} dB");
}

#[test]
fn a_blank_frame_is_video_black() {
    let gpu = gpu();
    let blank = Compositor::new(&gpu).blank((48, 24)).unwrap();
    let yuv = ToYuv::new(&gpu).convert(&blank).unwrap();
    assert!(yuv.luma.iter().all(|code| *code == 16));
    assert!(yuv.chroma.iter().all(|code| *code == 128));
}
