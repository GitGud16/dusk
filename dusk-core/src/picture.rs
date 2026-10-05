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
    /// `height` rows of `width` luma samples.
    pub luma: Vec<u8>,
    /// Rows of interleaved U and V samples, sized by [`Picture::chroma_size`].
    pub chroma: Vec<u8>,
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
