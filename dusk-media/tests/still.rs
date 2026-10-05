//! Still images: read once for their size and orientation, then decoded and scaled to the
//! size a render needs (docs/ARCHITECTURE.md, "Decoder pool").

use std::path::{Path, PathBuf};

use dusk_core::{ColorMatrix, ColorRange, Orientation, Picture, PictureLayout};
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
