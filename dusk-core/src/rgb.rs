//! Planar RGB pictures, for dusq's CPU transcode path (docs/ARCHITECTURE.md, "Compress tool
//! paths"): swscale normalizes every decoded frame to 16-bit RGB, color steps 3 and 4 and the
//! file's orientation turn it into upright 8-bit SDR BT.709, and swscale converts that for the
//! encoder.

use crate::color::{Primaries, SdrConverter, Transfer};
use crate::orientation::Orientation;

/// A picture as planar RGB with 16 bits a sample, full range, before color step 3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbPicture {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Red, green and blue, each `height` rows of `width` samples.
    pub planes: [Vec<u16>; 3],
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

impl RgbPicture {
    /// Color steps 3 and 4 and `orientation` for a band of the result: its rows from
    /// `first_row` on, as many as `band`'s planes hold, each as wide as the oriented picture.
    /// `converter` does step 3; `None` leaves the colors as they are, for BT.709 SDR.
    pub fn sdr_rows(
        &self,
        converter: Option<&SdrConverter>,
        orientation: Orientation,
        first_row: u32,
        band: [&mut [u8]; 3],
    ) {
        let width = orientation.apply_to_size((self.width, self.height)).0 as usize;
        let (origin, across, down) = source_steps(orientation, self.width, self.height);
        let [red, green, blue] = band;
        let rows = red.len().checked_div(width).unwrap_or(0);
        for row in 0..rows {
            let start = row * width;
            let mut source = origin + (first_row as usize + row) as isize * down;
            for at in start..start + width {
                // Within the picture: the steps were worked out from its own pixels.
                let index = source as usize;
                let rgb = [0, 1, 2].map(|plane| self.planes[plane][index]);
                let sdr = match converter {
                    Some(converter) => converter.convert(rgb).map(to_8_bits),
                    None => rgb.map(|value| ((u32::from(value) * 255 + 32_767) / 65_535) as u8),
                };
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
    // Saturates at 255; the converter's values stay within 0 to 1.
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

    /// A `width` by `height` BT.709 SDR picture whose samples count up, each plane from where
    /// the one before stopped, in steps that land on whole 8-bit codes.
    fn counting(width: u32, height: u32) -> RgbPicture {
        let count = (width * height) as u16;
        let plane = |offset: u16| (0..count).map(|n| (n + offset) * 257).collect();
        RgbPicture {
            width,
            height,
            planes: [plane(0), plane(count), plane(2 * count)],
            primaries: Primaries::Bt709,
            transfer: Transfer::Bt1886,
            peak_nits: 0,
        }
    }

    #[test]
    fn bt709_sdr_is_only_rounded_to_8_bits() {
        let picture = RgbPicture {
            width: 4,
            height: 1,
            planes: [
                vec![0, 128, 129, 65_535],
                vec![256, 385, 386, 32_767],
                vec![0; 4],
            ],
            primaries: Primaries::Bt709,
            transfer: Transfer::Bt1886,
            peak_nits: 0,
        };
        let sdr = picture.to_sdr(None, Orientation::UPRIGHT);
        assert_eq!((sdr.width, sdr.height), (4, 1));
        // A code is 257 apart: 128 rounds down to 0 and 129 up to 1, 385 down to 1 and 386
        // up to 2.
        assert_eq!(sdr.planes[0], [0, 0, 1, 255]);
        assert_eq!(sdr.planes[1], [1, 1, 2, 127]);
        assert_eq!(sdr.planes[2], [0; 4]);
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
                for (index, sample) in picture.planes[1].iter().enumerate() {
                    let (x, y) = (index as u32 % 3, index as u32 / 3);
                    let center = ((f64::from(x) + 0.5) / 3.0, (f64::from(y) + 0.5) / 2.0);
                    let (to_x, to_y) = orientation.map(center);
                    let to_x = (to_x * f64::from(sdr.width)).floor() as u32;
                    let to_y = (to_y * f64::from(sdr.height)).floor() as u32;
                    let landed = sdr.planes[1][(to_y * sdr.width + to_x) as usize];
                    assert_eq!(u16::from(landed), sample / 257, "{orientation:?} {x},{y}");
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
        let (top, bottom) = (width as usize, (width * (height - 1)) as usize);
        let (red_top, red_rest) = red.split_at_mut(top);
        let (green_top, green_rest) = green.split_at_mut(top);
        let (blue_top, blue_rest) = blue.split_at_mut(top);
        picture.sdr_rows(None, orientation, 0, [red_top, green_top, blue_top]);
        assert_eq!(red_rest.len(), bottom);
        picture.sdr_rows(None, orientation, 1, [red_rest, green_rest, blue_rest]);
        assert_eq!(planes, whole.planes);
    }

    #[test]
    fn step_3_applies_when_the_picture_needs_it() {
        let mut picture = counting(2, 2);
        picture.primaries = Primaries::Bt2020;
        picture.transfer = Transfer::Hlg;
        picture.peak_nits = 1000;
        let converter = SdrConverter::new(Primaries::Bt2020, Transfer::Hlg, 1000.0);
        let sdr = picture.to_sdr(converter.as_ref(), Orientation::UPRIGHT);
        let converter = converter.expect("HLG needs step 3");
        for index in 0..4 {
            let rgb = [0, 1, 2].map(|plane| picture.planes[plane][index]);
            let expected = converter
                .convert(rgb)
                .map(|value| (value * 255.0).round() as u8);
            let actual = [0, 1, 2].map(|plane| sdr.planes[plane][index]);
            assert_eq!(actual, expected, "pixel {index}");
        }
    }
}
