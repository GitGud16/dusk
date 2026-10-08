//! Thumbnails for the media bin (docs/ARCHITECTURE.md, "Memory discipline": their own cap of
//! 64 MB): one small picture of each video or photo, upright and in SDR BT.709, made on the
//! engine's thumbnail thread so the UI never waits for one.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::Sender;

use dusk_core::color::{DEFAULT_HDR_PEAK, to_sdr_bt709};
use dusk_core::{
    MediaId, MediaInfo, MediaKind, MediaTime, Orientation, Picture, PictureLayout, yuv_to_rgb,
};
use dusk_media::{Acceleration, VideoDecoder};

use crate::EngineError;
use crate::engine::{EngineEvent, Report, SharedTransport, lock};
use crate::info::still_size;

/// A small picture of a media file: 8-bit RGBA, rows packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The longest side of a thumbnail in pixels: twice the bin's 56 px, for high-DPI screens.
pub const THUMBNAIL_SIDE: u32 = 128;

/// A thumbnail to make: of media `media`, the file at `path` described by `info`.
pub(crate) struct ThumbnailJob {
    pub media: MediaId,
    pub path: PathBuf,
    pub info: MediaInfo,
}

/// While playback or an export runs, how often the thumbnail thread looks whether it may go on.
const WAIT: Duration = Duration::from_millis(100);

/// Starts the thumbnail thread. It makes thumbnails one at a time, in the order asked, and
/// reports each; files it cannot read get none, and playing them says why. It waits while
/// anything plays, since its decoder would be a third beside playback's two, and while an
/// export runs (docs/ARCHITECTURE.md, "Decoder pool" and "Export").
pub(crate) fn spawn(
    transport: SharedTransport,
    exporting: Arc<AtomicBool>,
    report: Report,
) -> Result<Sender<ThumbnailJob>, EngineError> {
    let (sender, inbox) = crossbeam_channel::unbounded::<ThumbnailJob>();
    std::thread::Builder::new()
        .name("dusk thumbnails".to_owned())
        .spawn(move || {
            for job in inbox {
                while exporting.load(Ordering::Relaxed) || lock(&transport).playing.is_some() {
                    std::thread::sleep(WAIT);
                }
                if let Ok(Some(thumbnail)) = thumbnail_of(&job.path, &job.info) {
                    report(EngineEvent::Thumbnail {
                        media: job.media,
                        thumbnail,
                    });
                }
            }
        })
        .map_err(EngineError::Thread)?;
    Ok(sender)
}

/// The thumbnail of the file at `path`, described by `info`; `None` for sound alone. A video
/// shows its keyframe at or before a tenth of the way in (at most a second in), which a
/// single decoded frame reaches; a photo is decoded just large enough.
pub fn thumbnail_of(path: &Path, info: &MediaInfo) -> Result<Option<Thumbnail>, EngineError> {
    let picture = match info.kind {
        MediaKind::Audio => return Ok(None),
        MediaKind::Still => {
            let side = THUMBNAIL_SIDE;
            dusk_media::decode_still(path, still_size(info, (side, side)))?
        }
        MediaKind::Video => {
            let mut decoder = VideoDecoder::open(path, Acceleration::Software)?;
            decoder.seek(MediaTime((info.duration.0 / 10).min(1_000_000)))?;
            match decoder.next_frame()? {
                Some(frame) => frame.picture,
                None => return Ok(None),
            }
        }
    };
    Ok(Some(shrink(&picture, info.orientation)))
}

/// `picture` shrunk to at most [`THUMBNAIL_SIDE`] on its longest side, turned upright by
/// `orientation`, in SDR BT.709 RGBA. Each pixel averages the samples it covers, then goes
/// through color steps 2 to 4 as the preview's do (docs/ARCHITECTURE.md).
fn shrink(picture: &Picture, orientation: Orientation) -> Thumbnail {
    let stored = (picture.width.max(1), picture.height.max(1));
    let upright = orientation.apply_to_size(stored);
    let scale = (f64::from(THUMBNAIL_SIDE) / f64::from(upright.0.max(upright.1))).min(1.0);
    let side = |length: u32| ((f64::from(length) * scale).round() as u32).max(1);
    let (width, height) = (side(upright.0), side(upright.1));
    let bits = match picture.layout {
        PictureLayout::Nv12 => 8,
        PictureLayout::P010 => 10,
    };
    let rows = yuv_to_rgb(picture.matrix, picture.range, bits);
    let peak = match picture.peak_nits {
        0 => DEFAULT_HDR_PEAK,
        nits => f64::from(nits),
    };
    // From the upright thumbnail back into the picture as stored.
    let back = orientation.inverse();
    let corner = |x: u32, y: u32| {
        back.map((
            f64::from(x) / f64::from(width),
            f64::from(y) / f64::from(height),
        ))
    };
    let planes = Planes::of(picture);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            // Quarter turns and mirrors keep the pixel's box a box.
            let (a, b) = (corner(x, y), corner(x + 1, y + 1));
            let columns = covered(a.0, b.0, stored.0);
            let lines = covered(a.1, b.1, stored.1);
            let luma = planes.luma(&columns, &lines);
            let (u, v) = planes.chroma(&columns, &lines);
            let rgb = rows.map(|[ky, ku, kv, constant]| {
                (ky * luma + ku * u + kv * v + constant).clamp(0.0, 1.0)
            });
            let rgb = to_sdr_bt709(rgb, picture.primaries, picture.transfer, peak);
            rgba.extend(rgb.map(|value| (value * 255.0).round() as u8));
            rgba.push(u8::MAX);
        }
    }
    Thumbnail {
        width,
        height,
        rgba,
    }
}

/// The whole samples between the fractions `a` and `b` of a side `length` samples long; at
/// least one.
fn covered(a: f64, b: f64, length: u32) -> Range<u32> {
    let (low, high) = (a.min(b) * f64::from(length), a.max(b) * f64::from(length));
    let start = (low.floor().max(0.0) as u32).min(length - 1);
    let end = (high.ceil() as u32).clamp(start + 1, length);
    start..end
}

/// A picture's planes, read as sample values.
struct Planes<'a> {
    picture: &'a Picture,
    /// Bytes per sample.
    bytes: usize,
}

impl Planes<'_> {
    fn of(picture: &Picture) -> Planes<'_> {
        Planes {
            picture,
            bytes: picture.layout.bytes_per_sample(),
        }
    }

    /// The sample at index `index` of `plane`; P010 keeps its value in the top 10 bits.
    fn sample(&self, plane: &[u8], index: usize) -> f64 {
        match self.bytes {
            1 => f64::from(plane[index]),
            _ => {
                let at = 2 * index;
                f64::from(u16::from_le_bytes([plane[at], plane[at + 1]]) >> 6)
            }
        }
    }

    /// The average luma over `columns` and `lines`.
    fn luma(&self, columns: &Range<u32>, lines: &Range<u32>) -> f64 {
        let width = self.picture.width as usize;
        let mut sum = 0.0;
        for line in lines.clone() {
            for column in columns.clone() {
                sum += self.sample(&self.picture.luma, line as usize * width + column as usize);
            }
        }
        sum / f64::from(columns.len() as u32 * lines.len() as u32)
    }

    /// The average U and V over the chroma samples under `columns` and `lines` of luma.
    fn chroma(&self, columns: &Range<u32>, lines: &Range<u32>) -> (f64, f64) {
        let (chroma_width, _) = self.picture.chroma_size();
        let chroma_columns = columns.start / 2..columns.end.div_ceil(2);
        let chroma_lines = lines.start / 2..lines.end.div_ceil(2);
        let (mut u, mut v) = (0.0, 0.0);
        for line in chroma_lines.clone() {
            for column in chroma_columns.clone() {
                let pair = 2 * (line as usize * chroma_width as usize + column as usize);
                u += self.sample(&self.picture.chroma, pair);
                v += self.sample(&self.picture.chroma, pair + 1);
            }
        }
        let count = f64::from(chroma_columns.len() as u32 * chroma_lines.len() as u32);
        (u / count, v / count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use dusk_core::color::{Primaries, Transfer};
    use dusk_core::{ColorMatrix, ColorRange, PictureLayout};

    /// An 8-bit BT.709 limited-range picture whose every pixel is `yuv`.
    fn uniform(width: u32, height: u32, yuv: [u8; 3]) -> Picture {
        let pairs = (width.div_ceil(2) * height.div_ceil(2)) as usize;
        Picture {
            width,
            height,
            layout: PictureLayout::Nv12,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            primaries: Primaries::Bt709,
            transfer: Transfer::Bt1886,
            peak_nits: 0,
            siting: dusk_core::ChromaSiting::LEFT,
            luma: vec![yuv[0]; (width * height) as usize],
            chroma: [yuv[1], yuv[2]].repeat(pairs),
        }
    }

    fn testdata(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../testdata")
            .join(name)
    }

    #[test]
    fn a_thumbnail_is_at_most_128_on_its_long_side() {
        let thumbnail = shrink(&uniform(320, 240, [126, 128, 128]), Orientation::UPRIGHT);
        assert_eq!((thumbnail.width, thumbnail.height), (128, 96));
        assert_eq!(thumbnail.rgba.len(), 128 * 96 * 4);
        // A small picture keeps its size.
        let small = shrink(&uniform(64, 36, [126, 128, 128]), Orientation::UPRIGHT);
        assert_eq!((small.width, small.height), (64, 36));
    }

    #[test]
    fn colors_come_out_as_rgb() {
        // (126 - 16) / 219 is half way.
        let gray = shrink(&uniform(32, 32, [126, 128, 128]), Orientation::UPRIGHT);
        assert_eq!(gray.rgba.len(), 32 * 32 * 4);
        assert!(
            gray.rgba
                .chunks(4)
                .all(|pixel| pixel == [128, 128, 128, 255])
        );
        let red = shrink(&uniform(32, 32, [63, 102, 240]), Orientation::UPRIGHT);
        assert!(
            red.rgba
                .chunks(4)
                .all(|p| p[0] >= 250 && p[1] <= 5 && p[2] <= 5 && p[3] == 255),
            "{:?}",
            &red.rgba[..4]
        );
    }

    #[test]
    fn a_turned_picture_comes_out_upright() {
        // Black with a white left half, stored to be turned a quarter clockwise.
        let mut picture = uniform(64, 32, [16, 128, 128]);
        for row in picture.luma.chunks_mut(64) {
            row[..32].fill(235);
        }
        let thumbnail = shrink(&picture, Orientation::new(1, false));
        assert_eq!((thumbnail.width, thumbnail.height), (32, 64));
        // The turn puts the stored left half on top.
        let red_at = |x: usize, y: usize| thumbnail.rgba[(y * 32 + x) * 4];
        assert_eq!(red_at(16, 10), 255);
        assert_eq!(red_at(16, 54), 0);
    }

    #[test]
    fn the_sample_video_gets_a_thumbnail() {
        let path = testdata("sample-h264-aac.mp4");
        let info = crate::media_info(&path).unwrap();
        let thumbnail = thumbnail_of(&path, &info).unwrap().expect("a picture");
        assert_eq!((thumbnail.width, thumbnail.height), (128, 96));
        // testsrc2 is colorful.
        assert!(
            thumbnail
                .rgba
                .chunks(4)
                .any(|pixel| pixel[..3] != [0, 0, 0])
        );
    }

    #[test]
    fn a_turned_photo_gets_an_upright_thumbnail() {
        let path = testdata("photo-turned.jpg");
        let info = crate::media_info(&path).unwrap();
        let thumbnail = thumbnail_of(&path, &info).unwrap().expect("a picture");
        assert_eq!((thumbnail.width, thumbnail.height), (96, 128));
    }

    #[test]
    fn sound_alone_has_no_thumbnail() {
        let path = testdata("chirp.wav");
        let info = crate::media_info(&path).unwrap();
        assert_eq!(thumbnail_of(&path, &info).unwrap(), None);
    }
}
