//! Needs a graphics adapter; CI runners use WARP (see tests/gpu.rs).

use dusk_core::{ColorMatrix, ColorRange, Picture, PictureLayout};
use dusk_render::{Compositor, Gpu};

fn compositor() -> Compositor {
    Compositor::new(&Gpu::new().expect("a graphics adapter: hardware, or WARP on CI"))
}

/// An 8-bit picture of one flat color.
fn flat(width: u32, height: u32, yuv: [u8; 3], matrix: ColorMatrix, range: ColorRange) -> Picture {
    let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
    Picture {
        width,
        height,
        layout: PictureLayout::Nv12,
        matrix,
        range,
        luma: vec![yuv[0]; (width * height) as usize],
        chroma: [yuv[1], yuv[2]].repeat((chroma_width * chroma_height) as usize),
    }
}

/// The same flat color as a 10-bit P010 picture.
fn flat_10_bit(
    width: u32,
    height: u32,
    yuv: [u16; 3],
    matrix: ColorMatrix,
    range: ColorRange,
) -> Picture {
    let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
    let sample = |code: u16| (code << 6).to_le_bytes();
    Picture {
        width,
        height,
        layout: PictureLayout::P010,
        matrix,
        range,
        luma: sample(yuv[0]).repeat((width * height) as usize),
        chroma: [sample(yuv[1]), sample(yuv[2])]
            .concat()
            .repeat((chroma_width * chroma_height) as usize),
    }
}

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * width + x) * 4) as usize;
    rgba[at..at + 4].try_into().unwrap()
}

fn close(actual: [u8; 4], expected: [u8; 4], tolerance: u8) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(a, e)| a.abs_diff(e) <= tolerance)
}

fn render(compositor: &Compositor, picture: &Picture, size: (u32, u32)) -> Vec<u8> {
    let texture = compositor
        .render(picture, size)
        .expect("the picture renders");
    assert_eq!((texture.width(), texture.height()), size);
    compositor
        .read_rgba(&texture)
        .expect("the frame reads back")
}

#[test]
fn the_output_is_a_texture_slint_can_show() {
    let compositor = compositor();
    let picture = flat(
        16,
        16,
        [126, 128, 128],
        ColorMatrix::Bt709,
        ColorRange::Limited,
    );
    let texture = compositor.render(&picture, (16, 16)).unwrap();
    assert_eq!(texture.format(), wgpu::TextureFormat::Rgba8Unorm);
    assert!(
        texture.usage().contains(
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT
        )
    );
}

#[test]
fn limited_range_black_and_white_reach_the_ends() {
    let compositor = compositor();
    for (y, expected) in [(16, [0, 0, 0, 255]), (235, [255, 255, 255, 255])] {
        let picture = flat(
            32,
            32,
            [y, 128, 128],
            ColorMatrix::Bt709,
            ColorRange::Limited,
        );
        let rgba = render(&compositor, &picture, (32, 32));
        assert!(close(pixel(&rgba, 32, 9, 21), expected, 1), "Y={y}");
    }
}

#[test]
fn each_matrix_turns_its_own_red_into_red() {
    let compositor = compositor();
    // The textbook codes for pure red in each matrix, limited range.
    for (matrix, yuv) in [
        (ColorMatrix::Bt601, [81, 90, 240]),
        (ColorMatrix::Bt709, [63, 102, 240]),
    ] {
        let picture = flat(32, 32, yuv, matrix, ColorRange::Limited);
        let rgba = render(&compositor, &picture, (32, 32));
        let red = pixel(&rgba, 32, 16, 16);
        assert!(close(red, [255, 0, 0, 255], 3), "{matrix:?}: {red:?}");
    }
}

#[test]
fn full_range_uses_every_code() {
    let compositor = compositor();
    for (y, expected) in [(0, [0, 0, 0, 255]), (255, [255, 255, 255, 255])] {
        let picture = flat(32, 32, [y, 128, 128], ColorMatrix::Bt709, ColorRange::Full);
        let rgba = render(&compositor, &picture, (32, 32));
        assert!(close(pixel(&rgba, 32, 3, 30), expected, 1), "Y={y}");
    }
}

#[test]
fn ten_bit_pictures_convert_like_eight_bit_ones() {
    let compositor = compositor();
    let eight = flat(
        32,
        32,
        [63, 102, 240],
        ColorMatrix::Bt709,
        ColorRange::Limited,
    );
    let ten = flat_10_bit(
        32,
        32,
        [252, 408, 960],
        ColorMatrix::Bt709,
        ColorRange::Limited,
    );
    let (a, b) = (
        render(&compositor, &eight, (32, 32)),
        render(&compositor, &ten, (32, 32)),
    );
    assert!(close(pixel(&a, 32, 7, 7), pixel(&b, 32, 7, 7), 1));
}

#[test]
fn a_wide_picture_is_fitted_between_black_bars() {
    let compositor = compositor();
    let gray = flat(
        64,
        32,
        [126, 128, 128],
        ColorMatrix::Bt709,
        ColorRange::Limited,
    );
    let rgba = render(&compositor, &gray, (64, 64));
    let black = [0, 0, 0, 255];
    let inside = pixel(&rgba, 64, 32, 32);
    assert!(close(inside, [128, 128, 128, 255], 2), "{inside:?}");
    for x in [0, 31, 63] {
        assert_eq!(pixel(&rgba, 64, x, 0), black);
        assert_eq!(pixel(&rgba, 64, x, 15), black);
        assert!(close(pixel(&rgba, 64, x, 16), inside, 1));
        assert!(close(pixel(&rgba, 64, x, 47), inside, 1));
        assert_eq!(pixel(&rgba, 64, x, 48), black);
    }
}

#[test]
fn scaling_down_keeps_a_flat_color_flat() {
    let compositor = compositor();
    let picture = flat(
        512,
        288,
        [63, 102, 240],
        ColorMatrix::Bt709,
        ColorRange::Limited,
    );
    // An eighth of the size, same shape: the widened kernel must still sum to one.
    let rgba = render(&compositor, &picture, (64, 36));
    let first = pixel(&rgba, 64, 0, 0);
    assert!(close(first, [255, 0, 0, 255], 3), "{first:?}");
    for y in 0..36 {
        for x in 0..64 {
            assert!(close(pixel(&rgba, 64, x, y), first, 1), "({x}, {y})");
        }
    }
}

#[test]
fn an_edge_stays_in_place_when_scaling_up() {
    let compositor = compositor();
    // Black on the left half, white on the right half.
    let mut picture = flat(
        64,
        16,
        [16, 128, 128],
        ColorMatrix::Bt709,
        ColorRange::Limited,
    );
    for row in picture.luma.chunks_mut(64) {
        row[32..].fill(235);
    }
    let rgba = render(&compositor, &picture, (128, 32));
    let level = |x| pixel(&rgba, 128, x, 16)[0];
    assert!(
        level(60) < 8 && level(67) > 247,
        "{} .. {}",
        level(60),
        level(67)
    );
    assert!(level(63) > 40 && level(63) < 215, "{}", level(63));
}

#[test]
fn a_frame_keeps_its_picture_until_two_newer_ones_are_drawn() {
    let compositor = compositor();
    let red = flat(
        16,
        16,
        [63, 102, 240],
        ColorMatrix::Bt709,
        ColorRange::Limited,
    );
    let gray = flat(
        16,
        16,
        [126, 128, 128],
        ColorMatrix::Bt709,
        ColorRange::Limited,
    );
    let first = compositor.render(&red, (16, 16)).unwrap();
    compositor.render(&gray, (16, 16)).unwrap();
    compositor.render(&gray, (16, 16)).unwrap();
    let kept = compositor.read_rgba(&first).unwrap();
    assert!(close(pixel(&kept, 16, 8, 8), [255, 0, 0, 255], 3));
    // The textures are reused in turn: the fourth frame is drawn into the first one.
    compositor.render(&gray, (16, 16)).unwrap();
    let reused = compositor.read_rgba(&first).unwrap();
    assert!(close(pixel(&reused, 16, 8, 8), [128, 128, 128, 255], 3));
}
