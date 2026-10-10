//! Color step 3 (docs/ARCHITECTURE.md, "Color and scaling"): sources whose
//! primaries are not BT.709 or whose transfer is HDR are linearized, tone-mapped when HDR,
//! moved into BT.709 and encoded again for 8-bit SDR output. These are the reference
//! functions: the compositor's shader does the same per pixel, and dusq's CPU path calls them.

/// A picture's color primaries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Primaries {
    /// BT.709 and sRGB; also what an untagged picture is taken to have.
    #[default]
    Bt709,
    /// BT.601 for 625-line systems (BT.470 B/G, PAL).
    Bt601_625,
    /// BT.601 for 525-line systems (SMPTE 170M, NTSC).
    Bt601_525,
    /// BT.2020, the primaries of UHD and HDR video.
    Bt2020,
    /// Display P3 (SMPTE EG 432-1, D65 white), as iPhone photos and videos use.
    DisplayP3,
}

/// How a picture's values relate to light.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Transfer {
    /// SDR video: BT.709, BT.601 and BT.2020 SDR, shown on a BT.1886 display (gamma 2.4).
    #[default]
    Bt1886,
    /// The sRGB curve, as photos use.
    Srgb,
    /// HDR: SMPTE ST 2084 (PQ), absolute light up to 10 000 nits.
    Pq,
    /// HDR: ARIB STD-B67 (HLG), relative scene light.
    Hlg,
}

impl Transfer {
    /// Whether the transfer is HDR, so tone mapping applies.
    pub fn is_hdr(self) -> bool {
        matches!(self, Transfer::Pq | Transfer::Hlg)
    }
}

/// The peak brightness SDR output stands for, in nits: HDR is tone-mapped down to it.
pub const SDR_PEAK: f64 = 100.0;

/// The peak an HDR source is taken to reach when its metadata says nothing plausible.
pub const DEFAULT_HDR_PEAK: f64 = 1000.0;

/// The source peak HDR tone mapping uses (docs/ARCHITECTURE.md): MaxCLL from the content
/// light level metadata when plausible (above 0 and at most 10 000 nits), else the mastering
/// display's maximum luminance, else [`DEFAULT_HDR_PEAK`].
pub fn source_peak(max_cll: Option<f64>, mastering_max: Option<f64>) -> f64 {
    let plausible = |nits: &f64| *nits > 0.0 && *nits <= 10_000.0;
    max_cll
        .filter(plausible)
        .or(mastering_max.filter(plausible))
        .unwrap_or(DEFAULT_HDR_PEAK)
}

/// The chromaticities (x, y) of red, green and blue.
fn chromaticities(primaries: Primaries) -> [(f64, f64); 3] {
    match primaries {
        Primaries::Bt709 => [(0.640, 0.330), (0.300, 0.600), (0.150, 0.060)],
        Primaries::Bt601_625 => [(0.640, 0.330), (0.290, 0.600), (0.150, 0.060)],
        Primaries::Bt601_525 => [(0.630, 0.340), (0.310, 0.595), (0.155, 0.070)],
        Primaries::Bt2020 => [(0.708, 0.292), (0.170, 0.797), (0.131, 0.046)],
        Primaries::DisplayP3 => [(0.680, 0.320), (0.265, 0.690), (0.150, 0.060)],
    }
}

/// D65, the white of every primaries above.
const D65: (f64, f64) = (0.3127, 0.3290);

type Matrix = [[f64; 3]; 3];

/// The matrix from linear RGB with `primaries` to CIE XYZ.
fn rgb_to_xyz(primaries: Primaries) -> Matrix {
    let xyz = |(x, y): (f64, f64)| [x / y, 1.0, (1.0 - x - y) / y];
    let columns = chromaticities(primaries).map(xyz);
    let unscaled = [0, 1, 2].map(|row| columns.map(|column| column[row]));
    // Each primary is scaled so that full red, green and blue add up to the white.
    let scale = multiply_vector(invert(unscaled), xyz(D65));
    unscaled.map(|row| [0, 1, 2].map(|column| row[column] * scale[column]))
}

fn multiply(a: Matrix, b: Matrix) -> Matrix {
    a.map(|row| [0, 1, 2].map(|column| (0..3).map(|k| row[k] * b[k][column]).sum()))
}

fn multiply_vector(matrix: Matrix, vector: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| row.iter().zip(vector).map(|(m, v)| m * v).sum())
}

fn invert(m: Matrix) -> Matrix {
    let cofactor = |row: usize, column: usize| {
        let (r0, r1) = ((row + 1) % 3, (row + 2) % 3);
        let (c0, c1) = ((column + 1) % 3, (column + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    let determinant: f64 = (0..3)
        .map(|column| m[0][column] * cofactor(0, column))
        .sum();
    // The inverse is the transposed cofactors over the determinant.
    [0, 1, 2].map(|row| [0, 1, 2].map(|column| cofactor(column, row) / determinant))
}

/// The matrix that turns linear RGB with `primaries` into linear BT.709 RGB (both with a D65
/// white), row by row. Values outside 0 to 1 are clipped after it, not by it.
pub fn to_bt709(primaries: Primaries) -> [[f64; 3]; 3] {
    multiply(invert(rgb_to_xyz(Primaries::Bt709)), rgb_to_xyz(primaries))
}

/// Linear light from 0 to 1 for an SDR value from 0 to 1 with `transfer` (BT.1886 or sRGB).
pub fn sdr_to_linear(transfer: Transfer, value: f64) -> f64 {
    let value = value.max(0.0);
    match transfer {
        Transfer::Srgb if value <= 0.04045 => value / 12.92,
        Transfer::Srgb => ((value + 0.055) / 1.055).powf(2.4),
        Transfer::Bt1886 | Transfer::Pq | Transfer::Hlg => value.powf(2.4),
    }
}

/// The SDR value from 0 to 1 for linear light from 0 to 1 with `transfer`.
pub fn linear_to_sdr(transfer: Transfer, light: f64) -> f64 {
    let light = light.max(0.0);
    match transfer {
        Transfer::Srgb if light <= 0.003_130_8 => light * 12.92,
        Transfer::Srgb => 1.055 * light.powf(1.0 / 2.4) - 0.055,
        Transfer::Bt1886 | Transfer::Pq | Transfer::Hlg => light.powf(1.0 / 2.4),
    }
}

// SMPTE ST 2084.
const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;

/// PQ's EOTF: light in nits for a PQ value from 0 to 1.
pub fn pq_to_nits(value: f64) -> f64 {
    let power = value.max(0.0).powf(1.0 / PQ_M2);
    let light = ((power - PQ_C1).max(0.0) / (PQ_C2 - PQ_C3 * power)).powf(1.0 / PQ_M1);
    10_000.0 * light
}

/// PQ's inverse EOTF: the PQ value from 0 to 1 for light in nits.
pub fn nits_to_pq(nits: f64) -> f64 {
    let power = (nits / 10_000.0).max(0.0).powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * power) / (1.0 + PQ_C3 * power)).powf(PQ_M2)
}

// ARIB STD-B67.
const HLG_A: f64 = 0.178_832_77;
const HLG_B: f64 = 0.284_668_92;
const HLG_C: f64 = 0.559_910_73;

/// HLG's inverse OETF: relative scene light from 0 to 1 for an HLG value from 0 to 1.
pub fn hlg_to_scene(value: f64) -> f64 {
    let value = value.max(0.0);
    if value <= 0.5 {
        value * value / 3.0
    } else {
        (((value - HLG_C) / HLG_A).exp() + HLG_B) / 12.0
    }
}

/// HLG's OOTF at the 1000-nit reference display (system gamma 1.2): display light in nits
/// for linear BT.2020 scene light.
pub fn hlg_ootf(scene: [f64; 3]) -> [f64; 3] {
    let [red, green, blue] = scene;
    let luminance = 0.2627 * red + 0.6780 * green + 0.0593 * blue;
    let gain = DEFAULT_HDR_PEAK * luminance.max(0.0).powf(0.2);
    scene.map(|light| gain * light)
}

/// BT.2390's EETF from a source peaking at `source_peak` nits down to [`SDR_PEAK`]: the PQ
/// value `value` mapped, a hermite roll-off above the knee and unchanged below it.
pub fn eetf(value: f64, source_peak: f64) -> f64 {
    let peak = nits_to_pq(source_peak);
    if peak <= 0.0 {
        return 0.0;
    }
    let normalized = (value / peak).clamp(0.0, 1.0);
    let max_luminance = nits_to_pq(SDR_PEAK) / peak;
    let knee = 1.5 * max_luminance - 0.5;
    let mapped = if normalized < knee {
        normalized
    } else {
        let t = (normalized - knee) / (1.0 - knee);
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * knee
            + (t3 - 2.0 * t2 + t) * (1.0 - knee)
            + (-2.0 * t3 + 3.0 * t2) * max_luminance
    };
    mapped * peak
}

/// Tone-maps linear BT.2020 display light in nits from a source peaking at `source_peak` to
/// SDR, as linear light where 1 is [`SDR_PEAK`]: max(R, G, B) goes through the EETF and all
/// three are scaled with it, which keeps hues.
pub fn tone_map(nits: [f64; 3], source_peak: f64) -> [f64; 3] {
    let nits = nits.map(|light| light.clamp(0.0, source_peak));
    let brightest = nits[0].max(nits[1]).max(nits[2]);
    if brightest <= 0.0 {
        return [0.0; 3];
    }
    let mapped = pq_to_nits(eetf(nits_to_pq(brightest), source_peak));
    let scale = mapped / brightest / SDR_PEAK;
    nits.map(|light| light * scale)
}

/// Color step 3 (docs/ARCHITECTURE.md) for one pixel of full-range RGB from 0 to 1 with
/// `primaries` and `transfer`: into SDR BT.709, HDR tone-mapped from `source_peak` nits (PQ;
/// HLG is shown at its 1000-nit reference). BT.709 SDR comes back as it is. The CPU twin of
/// the compositor's shader, for pictures too small to send to the GPU, such as thumbnails.
pub fn to_sdr_bt709(
    rgb: [f64; 3],
    primaries: Primaries,
    transfer: Transfer,
    source_peak: f64,
) -> [f64; 3] {
    if primaries == Primaries::Bt709 && !transfer.is_hdr() {
        return rgb;
    }
    let light = match transfer {
        Transfer::Pq => tone_map(rgb.map(pq_to_nits), source_peak),
        Transfer::Hlg => tone_map(hlg_ootf(rgb.map(hlg_to_scene)), DEFAULT_HDR_PEAK),
        Transfer::Bt1886 | Transfer::Srgb => rgb.map(|value| sdr_to_linear(transfer, value)),
    };
    let bt709 = to_bt709(primaries).map(|row| {
        let light = row[0] * light[0] + row[1] * light[1] + row[2] * light[2];
        light.clamp(0.0, 1.0)
    });
    // SDR goes back through its own curve; HDR comes out as SDR video, BT.1886.
    let curve = if transfer.is_hdr() {
        Transfer::Bt1886
    } else {
        transfer
    };
    bt709.map(|light| linear_to_sdr(curve, light))
}

/// Color step 3 for whole pictures on the CPU, as dusq's transcode path runs it on every pixel
/// (docs/ARCHITECTURE.md, "Compress tool paths"): [`to_sdr_bt709`] split into what acts on one
/// channel, worked out once into tables, and the little that needs the whole pixel. It stays
/// within a twentieth of an 8-bit code of the reference at a small part of its cost.
#[derive(Clone, Debug)]
pub struct SdrConverter {
    transfer: Transfer,
    /// Each 16-bit value as light: scene light for HLG, nits for PQ, linear light for SDR.
    decode: Vec<f32>,
    /// Linear light into BT.709.
    matrix: [[f32; 3]; 3],
    /// HDR: the peak, in nits, and BT.2390's knee, below which tone mapping only rescales.
    peak: f32,
    knee: f32,
    /// HDR: the scale for all three channels, by the brightest, from the knee to the peak,
    /// looked up by the square root of the way there, which puts more steps where the
    /// roll-off bends, just above the knee.
    tone: Vec<f32>,
    /// Linear BT.709 light back to SDR values, looked up by the light's square root, which
    /// keeps the steep start of the curve accurate.
    encode: Vec<f32>,
}

/// Steps in the tone and encoding tables of an [`SdrConverter`].
const TABLE_STEPS: usize = 4096;

impl SdrConverter {
    /// The converter for full-range RGB with `primaries` and `transfer`, HDR peaking at
    /// `source_peak` nits; `None` for BT.709 SDR, which step 3 leaves as it is.
    pub fn new(primaries: Primaries, transfer: Transfer, source_peak: f64) -> Option<SdrConverter> {
        if primaries == Primaries::Bt709 && !transfer.is_hdr() {
            return None;
        }
        // HLG is shown at its 1000-nit reference, as in `to_sdr_bt709`.
        let peak = if transfer == Transfer::Hlg {
            DEFAULT_HDR_PEAK
        } else {
            source_peak
        };
        let decode = (0..=u16::MAX)
            .map(|code| {
                let value = f64::from(code) / f64::from(u16::MAX);
                let light = match transfer {
                    Transfer::Pq => pq_to_nits(value),
                    Transfer::Hlg => hlg_to_scene(value),
                    Transfer::Bt1886 | Transfer::Srgb => sdr_to_linear(transfer, value),
                };
                light as f32
            })
            .collect();
        let (knee, tone) = if transfer.is_hdr() {
            // Where `eetf` starts to roll off, in nits.
            let peak_pq = nits_to_pq(peak);
            let max_luminance = nits_to_pq(SDR_PEAK) / peak_pq;
            let knee = pq_to_nits((1.5 * max_luminance - 0.5) * peak_pq).min(peak);
            let tone = (0..=TABLE_STEPS)
                .map(|step| {
                    let root = step as f64 / TABLE_STEPS as f64;
                    let brightest = knee + (peak - knee) * root * root;
                    let mapped = pq_to_nits(eetf(nits_to_pq(brightest), peak));
                    (mapped / brightest / SDR_PEAK) as f32
                })
                .collect();
            (knee, tone)
        } else {
            (0.0, Vec::new())
        };
        // SDR goes back through its own curve; HDR comes out as SDR video, BT.1886.
        let curve = if transfer.is_hdr() {
            Transfer::Bt1886
        } else {
            transfer
        };
        let encode = (0..=TABLE_STEPS)
            .map(|step| {
                let root = step as f64 / TABLE_STEPS as f64;
                linear_to_sdr(curve, root * root) as f32
            })
            .collect();
        Some(SdrConverter {
            transfer,
            decode,
            matrix: to_bt709(primaries).map(|row| row.map(|factor| factor as f32)),
            peak: peak as f32,
            knee: knee as f32,
            tone,
            encode,
        })
    }

    /// Step 3 for one pixel of full-range RGB from 0 to 1: SDR BT.709 values from 0 to 1.
    pub fn convert(&self, rgb: [f32; 3]) -> [f32; 3] {
        let last = self.decode.len() - 1;
        let mut light = rgb.map(|value| {
            let code = (value.clamp(0.0, 1.0) * last as f32 + 0.5) as usize;
            self.decode[code.min(last)]
        });
        if self.transfer == Transfer::Hlg {
            // `hlg_ootf`, at the 1000-nit reference.
            let [red, green, blue] = light;
            let luminance = 0.2627 * red + 0.6780 * green + 0.0593 * blue;
            let gain = DEFAULT_HDR_PEAK as f32 * luminance.max(0.0).powf(0.2);
            light = light.map(|scene| gain * scene);
        }
        if self.transfer.is_hdr() {
            // `tone_map`: what the EETF does to the brightest channel scales all three.
            light = light.map(|nits| nits.clamp(0.0, self.peak));
            let brightest = light[0].max(light[1]).max(light[2]);
            let scale = if brightest <= self.knee {
                1.0 / SDR_PEAK as f32
            } else {
                let way = (brightest - self.knee) / (self.peak - self.knee);
                interpolate(&self.tone, way.sqrt())
            };
            light = light.map(|nits| nits * scale);
        }
        self.matrix.map(|row| {
            let bt709 = row[0] * light[0] + row[1] * light[1] + row[2] * light[2];
            interpolate(&self.encode, bt709.clamp(0.0, 1.0).sqrt())
        })
    }
}

/// `table` read at `at`, from 0 for its first entry to 1 for its last, between entries
/// linearly.
fn interpolate(table: &[f32], at: f32) -> f32 {
    let last = table.len() - 1;
    let position = at.clamp(0.0, 1.0) * last as f32;
    let index = (position as usize).min(last - 1);
    let fraction = position - index as f32;
    table[index] + fraction * (table[index + 1] - table[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    fn apply(matrix: [[f64; 3]; 3], rgb: [f64; 3]) -> [f64; 3] {
        matrix.map(|row| row.iter().zip(rgb).map(|(m, v)| m * v).sum())
    }

    fn near_rgb(actual: [f64; 3], expected: [f64; 3], tolerance: f64) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(a, e)| (a - e).abs() <= tolerance)
    }

    #[test]
    fn bt709_sdr_skips_step_3() {
        let rgb = [0.2, 0.5, 0.8];
        assert_eq!(
            to_sdr_bt709(rgb, Primaries::Bt709, Transfer::Bt1886, 0.0),
            rgb
        );
        assert_eq!(
            to_sdr_bt709(rgb, Primaries::Bt709, Transfer::Srgb, 0.0),
            rgb
        );
    }

    #[test]
    fn a_display_p3_color_is_more_saturated_in_bt709() {
        // Gray stays gray; a warm color gets warmer, as P3's primaries lie outside BT.709's.
        let gray = to_sdr_bt709([0.5; 3], Primaries::DisplayP3, Transfer::Srgb, 0.0);
        assert!(near_rgb(gray, [0.5; 3], 1e-9), "{gray:?}");
        let [red, green, blue] =
            to_sdr_bt709([0.8, 0.5, 0.3], Primaries::DisplayP3, Transfer::Srgb, 0.0);
        assert!(
            red > 0.8 && green < 0.51 && blue < 0.3,
            "{red} {green} {blue}"
        );
    }

    #[test]
    fn hdr_peaks_reach_sdr_white_and_black_stays_black() {
        let peak = 1000.0;
        let pq_peak = [nits_to_pq(peak); 3];
        let white = to_sdr_bt709(pq_peak, Primaries::Bt2020, Transfer::Pq, peak);
        assert!(near_rgb(white, [1.0; 3], 1e-6), "{white:?}");
        let hlg_white = to_sdr_bt709([1.0; 3], Primaries::Bt2020, Transfer::Hlg, peak);
        assert!(near_rgb(hlg_white, [1.0; 3], 1e-6), "{hlg_white:?}");
        for transfer in [Transfer::Pq, Transfer::Hlg] {
            let black = to_sdr_bt709([0.0; 3], Primaries::Bt2020, transfer, peak);
            assert!(near_rgb(black, [0.0; 3], 1e-9), "{black:?}");
        }
        // Brighter in, brighter out: 100 nits stays below the peak's white.
        let reference = to_sdr_bt709(
            [nits_to_pq(100.0); 3],
            Primaries::Bt2020,
            Transfer::Pq,
            peak,
        );
        assert!(
            reference[0] > 0.5 && reference[0] < white[0],
            "{reference:?}"
        );
    }

    #[test]
    fn bt709_stays_as_it_is() {
        let identity = to_bt709(Primaries::Bt709);
        for (i, row) in identity.iter().enumerate() {
            for (j, value) in row.iter().enumerate() {
                assert!(near(*value, f64::from(u8::from(i == j)), 1e-12));
            }
        }
    }

    #[test]
    fn the_wide_gamuts_match_their_published_matrices() {
        let bt2020 = [
            [1.660_491, -0.587_641, -0.072_850],
            [-0.124_550, 1.132_900, -0.008_349],
            [-0.018_151, -0.100_579, 1.118_730],
        ];
        let p3 = [
            [1.224_940, -0.224_940, 0.0],
            [-0.042_057, 1.042_057, 0.0],
            [-0.019_638, -0.078_636, 1.098_274],
        ];
        for (primaries, expected) in [(Primaries::Bt2020, bt2020), (Primaries::DisplayP3, p3)] {
            let matrix = to_bt709(primaries);
            for (row, expected) in matrix.iter().zip(expected) {
                for (value, expected) in row.iter().zip(expected) {
                    assert!(near(*value, expected, 2e-4), "{primaries:?}: {matrix:?}");
                }
            }
        }
    }

    #[test]
    fn white_stays_white_in_every_gamut() {
        for primaries in [
            Primaries::Bt601_625,
            Primaries::Bt601_525,
            Primaries::Bt2020,
            Primaries::DisplayP3,
        ] {
            let white = apply(to_bt709(primaries), [1.0, 1.0, 1.0]);
            assert!(white.iter().all(|v| near(*v, 1.0, 1e-9)), "{primaries:?}");
        }
    }

    #[test]
    fn sdr_curves_round_trip_and_hit_their_known_points() {
        assert!(near(sdr_to_linear(Transfer::Srgb, 0.5), 0.214_041, 1e-6));
        assert!(near(
            sdr_to_linear(Transfer::Srgb, 0.04),
            0.04 / 12.92,
            1e-12
        ));
        assert!(near(
            sdr_to_linear(Transfer::Bt1886, 0.5),
            0.5f64.powf(2.4),
            1e-12
        ));
        for transfer in [Transfer::Srgb, Transfer::Bt1886] {
            for step in 0..=20 {
                let value = f64::from(step) / 20.0;
                let back = linear_to_sdr(transfer, sdr_to_linear(transfer, value));
                assert!(near(back, value, 1e-9), "{transfer:?} {value}");
            }
        }
    }

    #[test]
    fn pq_spans_ten_thousand_nits() {
        assert!(near(pq_to_nits(1.0), 10_000.0, 1e-6));
        assert!(near(pq_to_nits(0.0), 0.0, 1e-9));
        // Reference white for SDR, 100 nits, is about half the PQ range.
        assert!(near(nits_to_pq(100.0), 0.508_078, 1e-5));
        for nits in [0.5, 100.0, 1000.0, 4000.0] {
            assert!(near(pq_to_nits(nits_to_pq(nits)), nits, nits * 1e-9 + 1e-9));
        }
    }

    #[test]
    fn hlg_halfway_is_a_twelfth_of_scene_light() {
        assert!(near(hlg_to_scene(0.5), 1.0 / 12.0, 1e-9));
        assert!(near(hlg_to_scene(0.0), 0.0, 1e-12));
        assert!(near(hlg_to_scene(1.0), 1.0, 1e-4));
        // At the 1000-nit reference, full white scene light is shown at 1000 nits.
        let white = hlg_ootf([1.0, 1.0, 1.0]);
        assert!(white.iter().all(|v| near(*v, 1000.0, 1e-6)), "{white:?}");
    }

    #[test]
    fn the_eetf_keeps_the_shadows_and_rolls_the_peak_off_to_sdr_white() {
        let peak = 1000.0;
        let source = nits_to_pq(peak);
        assert!(near(eetf(source, peak), nits_to_pq(SDR_PEAK), 1e-9));
        let shadow = nits_to_pq(5.0);
        assert!(near(eetf(shadow, peak), shadow, 1e-12));
        let mut last = 0.0;
        for step in 0..=100 {
            let mapped = eetf(source * f64::from(step) / 100.0, peak);
            assert!(mapped >= last - 1e-12, "falls at step {step}");
            last = mapped;
        }
    }

    #[test]
    fn tone_mapping_brings_the_peak_to_sdr_white_and_keeps_hue() {
        let white = tone_map([1000.0, 1000.0, 1000.0], 1000.0);
        assert!(white.iter().all(|v| near(*v, 1.0, 1e-9)), "{white:?}");
        let orange = tone_map([800.0, 400.0, 0.0], 1000.0);
        assert!(near(orange[1] / orange[0], 0.5, 1e-9) && orange[2] == 0.0);
        assert!(orange[0] < 1.0);
        // Dim light is left alone, but for the change of scale.
        let dim = tone_map([20.0, 10.0, 5.0], 1000.0);
        assert!(near(dim[0], 0.2, 1e-9) && near(dim[2], 0.05, 1e-9));
    }

    /// A fixed spread of 16-bit colors: random ones, the gray ramp and the cube's corners.
    fn sample_colors() -> Vec<[u16; 3]> {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 48) as u16
        };
        let random = (0..20_000).map(|_| [next(), next(), next()]);
        let grays = (0..=256u32).map(|step| [(step * 65_535 / 256) as u16; 3]);
        let corners =
            (0..8).map(|corner: u16| [1, 2, 4].map(|bit| (corner & bit != 0) as u16 * u16::MAX));
        random.chain(grays).chain(corners).collect()
    }

    #[test]
    fn bt709_sdr_pictures_need_no_conversion() {
        assert!(SdrConverter::new(Primaries::Bt709, Transfer::Bt1886, 0.0).is_none());
        assert!(SdrConverter::new(Primaries::Bt709, Transfer::Srgb, 0.0).is_none());
        assert!(SdrConverter::new(Primaries::Bt709, Transfer::Hlg, 1000.0).is_some());
        assert!(SdrConverter::new(Primaries::DisplayP3, Transfer::Srgb, 0.0).is_some());
    }

    #[test]
    fn whole_pictures_convert_as_the_reference_does() {
        let sources = [
            (Primaries::Bt2020, Transfer::Hlg, DEFAULT_HDR_PEAK),
            (Primaries::Bt2020, Transfer::Pq, 1000.0),
            (Primaries::Bt2020, Transfer::Pq, 4000.0),
            (Primaries::Bt2020, Transfer::Pq, 80.0),
            (Primaries::Bt709, Transfer::Pq, 10_000.0),
            (Primaries::DisplayP3, Transfer::Srgb, 0.0),
            (Primaries::DisplayP3, Transfer::Bt1886, 0.0),
            (Primaries::Bt2020, Transfer::Bt1886, 0.0),
            (Primaries::Bt601_625, Transfer::Bt1886, 0.0),
        ];
        let colors = sample_colors();
        for (primaries, transfer, peak) in sources {
            let converter = SdrConverter::new(primaries, transfer, peak).expect("not BT.709 SDR");
            let mut worst = 0.0f64;
            for rgb in &colors {
                let reference = to_sdr_bt709(
                    rgb.map(|v| f64::from(v) / 65_535.0),
                    primaries,
                    transfer,
                    peak,
                );
                let converted = converter.convert(rgb.map(|v| f32::from(v) / 65_535.0));
                for (converted, reference) in converted.iter().zip(reference) {
                    worst = worst.max((f64::from(*converted) - reference).abs());
                }
            }
            // In 8-bit codes, a twentieth of a step at most.
            assert!(
                worst * 255.0 <= 0.05,
                "{primaries:?} {transfer:?} {peak}: {}",
                worst * 255.0
            );
        }
    }

    #[test]
    fn the_source_peak_trusts_plausible_metadata() {
        assert_eq!(source_peak(Some(1500.0), Some(4000.0)), 1500.0);
        assert_eq!(source_peak(Some(0.0), Some(4000.0)), 4000.0);
        assert_eq!(source_peak(Some(20_000.0), None), DEFAULT_HDR_PEAK);
        assert_eq!(source_peak(None, None), DEFAULT_HDR_PEAK);
    }
}
