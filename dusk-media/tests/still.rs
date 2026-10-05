//! Still images: read once for their size and orientation, then decoded and scaled to the
//! size a render needs (docs/ARCHITECTURE.md, "Decoder pool").

use std::path::{Path, PathBuf};

use dusk_core::color::{Primaries, Transfer};
use dusk_core::{ColorMatrix, ColorRange, Orientation, Picture, PictureLayout, yuv_to_rgb};
use dusk_media::{decode_still, is_still, probe, still_info};

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testdata")
        .join(name)
}

#[test]
fn photos_are_stills_and_videos_are_not() {
    assert!(is_still(&probe(&testdata("photo.png")).unwrap()));
    assert!(is_still(&probe(&testdata("photo-turned.jpg")).unwrap()));
    assert!(!is_still(&probe(&testdata("sample-h264-aac.mp4")).unwrap()));
}

/// The RGB of every pixel of an 8-bit NV12 picture, from 0 to 255.
fn rgb(picture: &Picture) -> Vec<[f64; 3]> {
    let rows = yuv_to_rgb(picture.matrix, picture.range, 8);
    let width = picture.width as usize;
    let chroma_width = picture.width.div_ceil(2) as usize;
    (0..picture.height as usize)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let luma = f64::from(picture.luma[y * width + x]);
            let pair = 2 * ((y / 2) * chroma_width + x / 2);
            let (u, v) = (
                f64::from(picture.chroma[pair]),
                f64::from(picture.chroma[pair + 1]),
            );
            rows.map(|[ky, ku, kv, constant]| 255.0 * (ky * luma + ku * u + kv * v + constant))
        })
        .collect()
}

#[test]
fn an_iphone_style_heic_grid_is_a_still() {
    assert!(is_still(&probe(&testdata("photo-grid.heic")).unwrap()));
}

#[test]
fn a_heic_grid_is_described_by_its_picture_not_its_tiles() {
    let grid = still_info(&testdata("photo-grid.heic")).unwrap();
    // Four tiles of 176x128 cropped to 320x240, to be turned a quarter clockwise (irot 3).
    assert_eq!((grid.width, grid.height), (320, 240));
    assert_eq!(grid.orientation, Orientation::new(1, false));
}

#[test]
fn a_heic_grid_reads_the_color_profile_of_its_grid() {
    // Like an iPhone photo's, the profile sits on the grid, not on the tiles.
    let grid = decode_still(&testdata("photo-grid.heic"), (160, 120)).unwrap();
    assert_eq!(grid.primaries, Primaries::DisplayP3);
    assert_eq!(grid.transfer, Transfer::Srgb);
}

#[test]
fn a_heic_grid_decodes_into_one_picture() {
    // photo.png, padded, cut into tiles and encoded as HEVC at a high quality: stitched and
    // cropped, it is that picture again.
    let grid = decode_still(&testdata("photo-grid.heic"), (320, 240)).unwrap();
    let png = decode_still(&testdata("photo.png"), (320, 240)).unwrap();
    let (grid, png) = (rgb(&grid), rgb(&png));
    let difference: f64 = grid
        .iter()
        .zip(&png)
        .flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| (a - b).abs()))
        .sum::<f64>()
        / (grid.len() * 3) as f64;
    assert!(difference < 4.0, "{difference} apart on average");
}

#[test]
fn a_photo_reads_its_stored_size_and_how_to_turn_it_upright() {
    let turned = still_info(&testdata("photo-turned.jpg")).unwrap();
    assert_eq!((turned.width, turned.height), (320, 240));
    // EXIF orientation 6: a quarter turn clockwise.
    assert_eq!(turned.orientation, Orientation::new(1, false));
    let plain = still_info(&testdata("photo.png")).unwrap();
    assert_eq!((plain.width, plain.height), (320, 240));
    assert_eq!(plain.orientation, Orientation::UPRIGHT);
    // 8-bit RGB: three bytes a pixel at the peak of decoding.
    assert!(plain.peak_bytes >= 320 * 240 * 3, "{}", plain.peak_bytes);
}

#[test]
fn a_photo_scales_to_the_size_asked() {
    for name in ["photo.png", "photo-turned.jpg"] {
        let picture = decode_still(&testdata(name), (160, 120)).unwrap();
        assert_eq!((picture.width, picture.height), (160, 120), "{name}");
        assert_eq!(picture.layout, PictureLayout::Nv12);
        assert_eq!(picture.luma.len(), 160 * 120);
        assert_eq!(picture.chroma.len(), 2 * 80 * 60);
    }
}

/// The RGB of pixel `x`, `y` of an NV12 picture, by its own matrix and range, from 0 to 1.
fn rgb_at(picture: &Picture, x: u32, y: u32) -> [f32; 3] {
    let luma = f32::from(picture.luma[(y * picture.width + x) as usize]);
    let at = (2 * ((y / 2) * picture.chroma_size().0 + x / 2)) as usize;
    let (u, v) = (
        f32::from(picture.chroma[at]),
        f32::from(picture.chroma[at + 1]),
    );
    let (y, u, v) = match picture.range {
        ColorRange::Full => (luma / 255.0, (u - 128.0) / 255.0, (v - 128.0) / 255.0),
        ColorRange::Limited => (
            (luma - 16.0) / 219.0,
            (u - 128.0) / 224.0,
            (v - 128.0) / 224.0,
        ),
    };
    let (kr, kb) = match picture.matrix {
        ColorMatrix::Bt601 => (0.299, 0.114),
        ColorMatrix::Bt709 => (0.2126, 0.0722),
        ColorMatrix::Bt2020 => (0.2627, 0.0593),
    };
    let kg = 1.0 - kr - kb;
    let r = y + 2.0 * (1.0 - kr) * v;
    let b = y + 2.0 * (1.0 - kb) * u;
    let g = (y - kr * r - kb * b) / kg;
    [r, g, b]
}

#[test]
fn a_png_and_a_jpeg_of_the_same_picture_come_out_alike() {
    // The same test pattern: RGB in the PNG, full-range BT.601 YUV in the JPEG. Each picture
    // is tagged with its own matrix and range, so both decode to the same colors.
    let png = decode_still(&testdata("photo.png"), (320, 240)).unwrap();
    let jpeg = decode_still(&testdata("photo-turned.jpg"), (320, 240)).unwrap();
    let mut differences = Vec::new();
    for y in (20..220).step_by(10) {
        for x in (20..300).step_by(10) {
            let (a, b) = (rgb_at(&png, x, y), rgb_at(&jpeg, x, y));
            differences.extend(a.iter().zip(b).map(|(a, b)| (a - b).abs()));
        }
    }
    let mean = differences.iter().sum::<f32>() / differences.len() as f32;
    assert!(mean < 0.03, "mean difference {mean}");
}

#[test]
fn a_file_that_is_not_a_picture_is_refused() {
    let toml = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    assert!(still_info(&toml).is_err());
    assert!(decode_still(&toml, (16, 16)).is_err());
}

#[test]
fn a_photo_says_which_colors_it_holds() {
    // An embedded Display P3 profile, read through FFmpeg's ICC support.
    let p3 = decode_still(&testdata("photo-p3.jpg"), (160, 120)).unwrap();
    assert_eq!(
        (p3.primaries, p3.transfer),
        (Primaries::DisplayP3, Transfer::Srgb)
    );
    // Without a profile, a photo is sRGB.
    let plain = decode_still(&testdata("photo.png"), (160, 120)).unwrap();
    assert_eq!(
        (plain.primaries, plain.transfer),
        (Primaries::Bt709, Transfer::Srgb)
    );
    assert_eq!(plain.peak_nits, 0);
}
