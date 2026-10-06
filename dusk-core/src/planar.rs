//! Planar pictures for dusq's CPU transcode path (docs/ARCHITECTURE.md, "Compress tool
//! paths"): swscale resamples every decoded frame to 16-bit YUV 4:4:4 of the output size
//! (color step 1); here steps 2 to 4 and the file's orientation turn it into upright 8-bit SDR
//! BT.709 RGB, which swscale converts for the encoder.

use crate::color::{Primaries, SdrConverter, Transfer};
use crate::orientation::Orientation;
use crate::picture::{ColorMatrix, ColorRange, yuv_to_rgb};

/// A decoded frame as the CPU path normalizes it: planar YUV 4:4:4 with 16 bits a sample.
/// swscale keeps the source's matrix and range and only shifts its values up, so a sample is
/// the source's value times 2 to the power of (16 − `bits`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YuvPicture {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Y, U and V, each `height` rows of `width` samples.
    pub planes: [Vec<u16>; 3],
    /// The YUV-to-RGB matrix.
    pub matrix: ColorMatrix,
    /// The YUV value range.
    pub range: ColorRange,
    /// Bits a sample in the source, from 8 to 16: what the values were shifted up from.
    pub bits: u32,
    /// The color primaries.
    pub primaries: Primaries,
    /// How the values relate to light.
    pub transfer: Transfer,
    /// For an HDR picture, the brightest it gets in nits; 0 for SDR.
    pub peak_nits: u16,
}

/// An upright SDR BT.709 picture as planar RGB with 8 bits a sample, full range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdrPicture {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Red, green and blue, each `height` rows of `width` samples.
    pub planes: [Vec<u8>; 3],
}

impl YuvPicture {
    /// Color steps 2 to 4 and `orientation` for a band of the result: its rows from
    /// `first_row` on, as many as `band`'s planes hold, each as wide as the oriented picture.
    /// `converter` does step 3; `None` skips it, for BT.709 SDR.
    pub fn sdr_rows(
        &self,
        converter: Option<&SdrConverter>,
        orientation: Orientation,
        first_row: u32,
        band: [&mut [u8]; 3],
    ) {
        let width = orientation.apply_to_size((self.width, self.height)).0 as usize;
        let (origin, across, down) = source_steps(orientation, self.width, self.height);
        // Step 2's factors for values in the source's own bits, scaled for the shifted ones.
        let bits = self.bits.clamp(8, 16);
        let shift = f64::from(1u32 << (16 - bits));
        let factors = yuv_to_rgb(self.matrix, self.range, bits).map(|[y, u, v, constant]| {
            [y / shift, u / shift, v / shift, constant].map(|f| f as f32)
        });
        let [red, green, blue] = band;
        let rows = red.len().checked_div(width).unwrap_or(0);
        for row in 0..rows {
            let start = row * width;
            let mut source = origin + (first_row as usize + row) as isize * down;
            for at in start..start + width {
                // Within the picture: the steps were worked out from its own pixels.
                let index = source as usize;
                let [y, u, v] = [0, 1, 2].map(|plane| f32::from(self.planes[plane][index]));
                let rgb = factors.map(|[for_y, for_u, for_v, constant]| {
                    (for_y * y + for_u * u + for_v * v + constant).clamp(0.0, 1.0)
                });
                let sdr = match converter {
                    Some(converter) => converter.convert(rgb),
                    None => rgb,
                }
                .map(to_8_bits);
                (red[at], green[at], blue[at]) = (sdr[0], sdr[1], sdr[2]);
                source += across;
            }
        }
    }

    /// The whole picture through [`sdr_rows`](Self::sdr_rows) at once.
    pub fn to_sdr(&self, converter: Option<&SdrConverter>, orientation: Orientation) -> SdrPicture {
        let (width, height) = orientation.apply_to_size((self.width, self.height));
        let mut planes = [0; 3].map(|_| vec![0u8; width as usize * height as usize]);
        let [red, green, blue] = &mut planes;
        self.sdr_rows(converter, orientation, 0, [red, green, blue]);
        SdrPicture {
            width,
            height,
            planes,
        }
    }
}

/// Color step 4: a value from 0 to 1 as the nearest 8-bit code.
fn to_8_bits(value: f32) -> u8 {
    // Saturates at 255; the values stay within 0 to 1.
    (value * 255.0 + 0.5) as u8
}

/// Where the pixels of a `width` by `height` picture oriented by `orientation` come from:
/// the index of the top-left one in the source planes, and the steps from one to the next
/// across and down. Pixel centers map to pixel centers, so the steps are whole.
fn source_steps(orientation: Orientation, width: u32, height: u32) -> (isize, isize, isize) {
    let (oriented_width, oriented_height) = orientation.apply_to_size((width, height));
    let back = orientation.inverse();
    let index = |x: f64, y: f64| {
        let center = (
            (x + 0.5) / f64::from(oriented_width),
            (y + 0.5) / f64::from(oriented_height),
        );
        let (from_x, from_y) = back.map(center);
        let column = (from_x * f64::from(width)).floor() as isize;
        let row = (from_y * f64::from(height)).floor() as isize;
        row * width as isize + column
    };
    let origin = index(0.0, 0.0);
    (origin, index(1.0, 0.0) - origin, index(0.0, 1.0) - origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width` by `height` picture of `bits`-bit values shifted up to 16 bits, Y, U and V
    /// at `yuv` everywhere, untagged SDR.
    fn flat(
        width: u32,
        height: u32,
        yuv: [u16; 3],
        matrix: ColorMatrix,
        range: ColorRange,
        bits: u32,
    ) -> YuvPicture {
        let count = (width * height) as usize;
        YuvPicture {
            width,
            height,
            planes: yuv.map(|value| vec![value << (16 - bits); count]),
            matrix,
            range,
            bits,
            primaries: Primaries::Bt709,
            transfer: Transfer::Bt1886,
            peak_nits: 0,
        }
    }

    fn first_pixel(sdr: &SdrPicture) -> [u8; 3] {
        [0, 1, 2].map(|plane| sdr.planes[plane][0])
    }

    #[test]
    fn video_range_bt709_turns_into_rgb() {
        let rgb = |yuv| {
            let picture = flat(2, 2, yuv, ColorMatrix::Bt709, ColorRange::Limited, 8);
            first_pixel(&picture.to_sdr(None, Orientation::UPRIGHT))
        };
        assert_eq!(rgb([235, 128, 128]), [255, 255, 255]);
        assert_eq!(rgb([16, 128, 128]), [0, 0, 0]);
        assert_eq!(rgb([126, 128, 128]), [128, 128, 128]);
        // BT.709 red: 63, 102, 240.
        let [red, green, blue] = rgb([63, 102, 240]);
        assert!(
            red >= 254 && green <= 1 && blue <= 1,
            "{red} {green} {blue}"
        );
    }

    #[test]
    fn full_range_white_stays_white_at_any_bit_depth() {
        // swscale shifts values up: 8-bit 255 becomes 65,280, short of 65,535.
        let eight = flat(
            1,
            1,
            [255, 128, 128],
            ColorMatrix::Bt601,
            ColorRange::Full,
            8,
        );
        assert_eq!(
            first_pixel(&eight.to_sdr(None, Orientation::UPRIGHT)),
            [255; 3]
        );
        let ten = flat(
            1,
            1,
            [1023, 512, 512],
            ColorMatrix::Bt2020,
            ColorRange::Full,
            10,
        );
        assert_eq!(
            first_pixel(&ten.to_sdr(None, Orientation::UPRIGHT)),
            [255; 3]
        );
        let video = flat(
            1,
            1,
            [940, 512, 512],
            ColorMatrix::Bt2020,
            ColorRange::Limited,
            10,
        );
        assert_eq!(
            first_pixel(&video.to_sdr(None, Orientation::UPRIGHT)),
            [255; 3]
        );
    }

    /// A full-range gray picture whose pixels count up, so each comes out as its own code.
    fn counting(width: u32, height: u32) -> YuvPicture {
        let mut picture = flat(
            width,
            height,
            [0, 128, 128],
            ColorMatrix::Bt709,
            ColorRange::Full,
            8,
        );
        picture.planes[0] = (0..(width * height) as u16)
            .map(|n| (n * 10) << 8)
            .collect();
        picture
    }

    #[test]
    fn every_pixel_lands_where_the_orientation_puts_it() {
        let picture = counting(3, 2);
        for turns in 0..4 {
            for mirrored in [false, true] {
                let orientation = Orientation::new(turns, mirrored);
                let sdr = picture.to_sdr(None, orientation);
                assert_eq!(
                    (sdr.width, sdr.height),
                    orientation.apply_to_size((3, 2)),
                    "{orientation:?}"
                );
                for (index, sample) in picture.planes[0].iter().enumerate() {
                    let (x, y) = (index as u32 % 3, index as u32 / 3);
                    let center = ((f64::from(x) + 0.5) / 3.0, (f64::from(y) + 0.5) / 2.0);
                    let (to_x, to_y) = orientation.map(center);
                    let to_x = (to_x * f64::from(sdr.width)).floor() as u32;
                    let to_y = (to_y * f64::from(sdr.height)).floor() as u32;
                    let landed = sdr.planes[1][(to_y * sdr.width + to_x) as usize];
                    assert_eq!(u16::from(landed), sample >> 8, "{orientation:?} {x},{y}");
                }
            }
        }
    }

    #[test]
    fn bands_of_rows_make_the_whole_picture() {
        let picture = counting(4, 5);
        let orientation = Orientation::new(1, true);
        let whole = picture.to_sdr(None, orientation);
        let (width, height) = orientation.apply_to_size((4, 5));
        let mut planes = [0; 3].map(|_| vec![0u8; (width * height) as usize]);
        let [red, green, blue] = &mut planes;
        let (top, rest) = (width as usize, (width * (height - 1)) as usize);
        let (red_top, red_rest) = red.split_at_mut(top);
        let (green_top, green_rest) = green.split_at_mut(top);
        let (blue_top, blue_rest) = blue.split_at_mut(top);
        picture.sdr_rows(None, orientation, 0, [red_top, green_top, blue_top]);
        assert_eq!(red_rest.len(), rest);
        picture.sdr_rows(None, orientation, 1, [red_rest, green_rest, blue_rest]);
        assert_eq!(planes, whole.planes);
    }

    #[test]
    fn step_3_applies_when_the_picture_needs_it() {
        let mut picture = flat(
            2,
            1,
            [600, 400, 700],
            ColorMatrix::Bt2020,
            ColorRange::Limited,
            10,
        );
        picture.planes[0][1] = 900 << 6;
        picture.primaries = Primaries::Bt2020;
        picture.transfer = Transfer::Hlg;
        picture.peak_nits = 1000;
        let converter = SdrConverter::new(Primaries::Bt2020, Transfer::Hlg, 1000.0);
        let sdr = picture.to_sdr(converter.as_ref(), Orientation::UPRIGHT);
        let converter = converter.expect("HLG needs step 3");
        let rows = yuv_to_rgb(ColorMatrix::Bt2020, ColorRange::Limited, 10);
        for index in 0..2 {
            let yuv = [0, 1, 2].map(|plane| f64::from(picture.planes[plane][index] >> 6));
            let rgb = rows.map(|[a, b, c, d]| {
                (a * yuv[0] + b * yuv[1] + c * yuv[2] + d).clamp(0.0, 1.0) as f32
            });
            let expected = converter.convert(rgb).map(to_8_bits);
            let actual = [0, 1, 2].map(|plane| sdr.planes[plane][index]);
            assert_eq!(actual, expected, "pixel {index}");
        }
    }
}
