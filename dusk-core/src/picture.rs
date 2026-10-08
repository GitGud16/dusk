//! Decoded pictures: the plain data `dusk-media` decodes into, the frame cache holds and
//! `dusk-render` draws (docs/ARCHITECTURE.md, "Memory discipline").

use crate::color::{Primaries, Transfer};

/// How a picture's samples are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureLayout {
    /// 8-bit 4:2:0: a luma plane of bytes, then a plane of interleaved U and V bytes at half
    /// the width and height.
    Nv12,
    /// 10-bit 4:2:0: the same planes as NV12 in 16-bit little-endian samples, with the value
    /// in the top 10 bits.
    P010,
}

impl PictureLayout {
    /// Bytes per sample.
    pub fn bytes_per_sample(self) -> usize {
        match self {
            PictureLayout::Nv12 => 1,
            PictureLayout::P010 => 2,
        }
    }
}

/// The matrix that turns a picture's YUV values into RGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMatrix {
    /// Standard-definition video.
    Bt601,
    /// High-definition video.
    Bt709,
    /// UHD and HDR video.
    Bt2020,
}

/// Which range a picture's YUV values use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorRange {
    /// Video range: luma from 16 to 235 (scaled for 10-bit).
    Limited,
    /// The full range, as in JPEG and many screen recordings.
    Full,
}

/// Where a 4:2:0 picture's chroma samples sit, in 256ths of a luma pixel from the top-left
/// one of the two by two luma pixels each covers: across, then down, as FFmpeg counts chroma
/// positions. Step 1 of color and scaling reads chroma there (docs/ARCHITECTURE.md, "Decoder
/// pool").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChromaSiting {
    /// From 0, on the left luma column, to 256, on the right one.
    pub across: i32,
    /// From 0, on the top luma row, to 256, on the bottom one.
    pub down: i32,
}

impl ChromaSiting {
    /// MPEG-2's, which most video uses and Dusk writes: on the even luma columns, halfway
    /// between two rows.
    pub const LEFT: ChromaSiting = ChromaSiting {
        across: 0,
        down: 128,
    };
    /// JPEG's: in the middle of its four luma pixels.
    pub const CENTER: ChromaSiting = ChromaSiting {
        across: 128,
        down: 128,
    };
}

/// A decoded picture in memory Dusk owns, its rows packed without padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How the samples are stored.
    pub layout: PictureLayout,
    /// The YUV-to-RGB matrix.
    pub matrix: ColorMatrix,
    /// The YUV value range.
    pub range: ColorRange,
    /// The color primaries.
    pub primaries: Primaries,
    /// How the values relate to light.
    pub transfer: Transfer,
    /// For an HDR picture, the brightest it gets in nits, where tone mapping starts from
    /// (`color::source_peak`); 0 for SDR.
    pub peak_nits: u16,
    /// Where its chroma sits.
    pub siting: ChromaSiting,
    /// `height` rows of `width` luma samples.
    pub luma: Vec<u8>,
    /// Rows of interleaved U and V samples, sized by [`Picture::chroma_size`].
    pub chroma: Vec<u8>,
}

/// The rows of the affine map from `bits`-bit YUV sample values to full-range RGB from 0 to
/// 1 for `matrix` and `range` (docs/ARCHITECTURE.md, color and scaling step 2): each row
/// holds the factors for Y, U and V, then a constant.
pub fn yuv_to_rgb(matrix: ColorMatrix, range: ColorRange, bits: u32) -> [[f64; 4]; 3] {
    let (kr, kb) = match matrix {
        ColorMatrix::Bt601 => (0.299, 0.114),
        ColorMatrix::Bt709 => (0.2126, 0.0722),
        ColorMatrix::Bt2020 => (0.2627, 0.0593),
    };
    let kg = 1.0 - kr - kb;
    let max = f64::from((1u32 << bits) - 1);
    let k = f64::from(1u32 << (bits - 8));
    // Y' = (Y - y_zero) / y_span; Cb and Cr = (U or V - c_zero) / c_span.
    let (y_zero, y_span, c_zero, c_span) = match range {
        ColorRange::Limited => (16.0 * k, 219.0 * k, 128.0 * k, 224.0 * k),
        ColorRange::Full => (0.0, max, f64::from(1u32 << (bits - 1)), max),
    };
    let rows: [[f64; 3]; 3] = [
        [1.0, 0.0, 2.0 * (1.0 - kr)],
        [
            1.0,
            -2.0 * kb * (1.0 - kb) / kg,
            -2.0 * kr * (1.0 - kr) / kg,
        ],
        [1.0, 2.0 * (1.0 - kb), 0.0],
    ];
    rows.map(|[y, cb, cr]| {
        let (y, cb, cr) = (y / y_span, cb / c_span, cr / c_span);
        let constant = -y * y_zero - (cb + cr) * c_zero;
        [y, cb, cr, constant]
    })
}

impl Picture {
    /// The chroma plane's width (in U, V pairs) and height: half the picture's, rounded up.
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }

    /// How many bytes the planes hold, which is what the frame cache counts.
    pub fn byte_size(&self) -> usize {
        self.luma.len() + self.chroma.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(rows: [[f64; 4]; 3], yuv: [f64; 3]) -> [f64; 3] {
        rows.map(|[a, b, c, d]| a * yuv[0] + b * yuv[1] + c * yuv[2] + d)
    }

    fn near(actual: [f64; 3], expected: [f64; 3], tolerance: f64) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(a, e)| (a - e).abs() <= tolerance)
    }

    #[test]
    fn limited_range_maps_16_and_235_to_black_and_white() {
        let rows = yuv_to_rgb(ColorMatrix::Bt709, ColorRange::Limited, 8);
        assert!(near(apply(rows, [16.0, 128.0, 128.0]), [0.0; 3], 1e-9));
        assert!(near(apply(rows, [235.0, 128.0, 128.0]), [1.0; 3], 1e-9));
    }

    #[test]
    fn the_textbook_reds_come_out_red() {
        let bt601 = yuv_to_rgb(ColorMatrix::Bt601, ColorRange::Limited, 8);
        let bt709 = yuv_to_rgb(ColorMatrix::Bt709, ColorRange::Limited, 8);
        assert!(near(
            apply(bt601, [81.0, 90.0, 240.0]),
            [1.0, 0.0, 0.0],
            0.01
        ));
        assert!(near(
            apply(bt709, [63.0, 102.0, 240.0]),
            [1.0, 0.0, 0.0],
            0.01
        ));
    }

    #[test]
    fn ten_bit_codes_are_four_times_eight_bit_ones() {
        let eight = yuv_to_rgb(ColorMatrix::Bt709, ColorRange::Limited, 8);
        let ten = yuv_to_rgb(ColorMatrix::Bt709, ColorRange::Limited, 10);
        let rgb = apply(eight, [63.0, 102.0, 240.0]);
        assert!(near(apply(ten, [252.0, 408.0, 960.0]), rgb, 1e-9));
    }

    #[test]
    fn full_range_spans_every_code() {
        let rows = yuv_to_rgb(ColorMatrix::Bt601, ColorRange::Full, 8);
        assert!(near(apply(rows, [0.0, 128.0, 128.0]), [0.0; 3], 1e-9));
        assert!(near(apply(rows, [255.0, 128.0, 128.0]), [1.0; 3], 1e-9));
    }

    fn picture(width: u32, height: u32, layout: PictureLayout) -> Picture {
        let samples = width as usize * height as usize * layout.bytes_per_sample();
        let chroma = (width.div_ceil(2) * height.div_ceil(2) * 2) as usize;
        Picture {
            width,
            height,
            layout,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            primaries: crate::color::Primaries::Bt709,
            transfer: crate::color::Transfer::Bt1886,
            peak_nits: 0,
            siting: ChromaSiting::LEFT,
            luma: vec![0; samples],
            chroma: vec![0; chroma * layout.bytes_per_sample()],
        }
    }

    #[test]
    fn chroma_is_half_size_rounded_up() {
        assert_eq!(
            picture(1920, 1080, PictureLayout::Nv12).chroma_size(),
            (960, 540)
        );
        assert_eq!(
            picture(321, 241, PictureLayout::Nv12).chroma_size(),
            (161, 121)
        );
    }

    #[test]
    fn the_byte_size_counts_both_planes() {
        assert_eq!(
            picture(1920, 1080, PictureLayout::Nv12).byte_size(),
            3_110_400
        );
        assert_eq!(
            picture(1920, 1080, PictureLayout::P010).byte_size(),
            6_220_800
        );
    }
}
