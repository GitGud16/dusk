//! Still images (docs/ARCHITECTURE.md, "Decoder pool"): JPEG, PNG, WebP, TIFF and the other
//! pictures FFmpeg reads. A still is decoded once at import for its size and orientation,
//! then decoded again whenever a render needs it, scaled at once through swscale to the size
//! that render draws it at, and the decoded frame is released: only the scaled picture stays.

use std::path::Path;

use dusk_core::{ColorMatrix, ColorRange, Orientation, Picture, PictureLayout};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::frame;

use crate::ffi::{self, ScaleColors};
use crate::input::open_input;
use crate::orientation::from_display_matrix;
use crate::{MediaError, ProbeInfo};

/// What a still image holds, read by decoding it once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StillInfo {
    /// The picture's width as stored, before it is turned upright.
    pub width: u32,
    /// The picture's height as stored.
    pub height: u32,
    /// How to turn it upright, from its EXIF orientation.
    pub orientation: Orientation,
    /// The most memory decoding it at full size takes, in bytes: the decoded frame in its
    /// own pixel format, and for a progressive JPEG the coefficients the decoder keeps too.
    pub peak_bytes: u64,
}

/// Whether a probed file is a still image rather than video or sound: one picture, read by
/// FFmpeg's image demuxers.
pub fn is_still(probe: &ProbeInfo) -> bool {
    probe.format == "image2" || probe.format.ends_with("_pipe")
}

/// Decodes the still image at `path` once and describes it.
pub fn still_info(path: &Path) -> Result<StillInfo, MediaError> {
    // The smallest size JPEG decodes at is enough to learn what the file holds.
    let decoded = decode_first(path, None)?;
    let orientation = ffi::frame_display_matrix(&decoded.frame)
        .map_or(Orientation::UPRIGHT, |matrix| from_display_matrix(&matrix));
    let frame_bytes = ffi::frame_bytes(decoded.frame.format(), decoded.width, decoded.height);
    // A progressive JPEG decoder holds the picture's coefficients besides the frame: about
    // twice the frame again.
    let peak_bytes = if decoded.progressive {
        frame_bytes.saturating_mul(3)
    } else {
        frame_bytes
    };
    Ok(StillInfo {
        width: decoded.width,
        height: decoded.height,
        orientation,
        peak_bytes,
    })
}

/// Decodes the still image at `path` and scales it to `size` (width, height, as stored) as
/// an NV12 picture tagged with its own matrix and range.
pub fn decode_still(path: &Path, size: (u32, u32)) -> Result<Picture, MediaError> {
    let decoded = decode_first(path, Some(size))?;
    let colors = colors_of(&decoded.frame);
    let (luma, chroma) = ffi::scale_to_nv12(&decoded.frame, size, &colors)
        .map_err(|source| decode_error(path, source))?;
    // The decoded frame goes here; only the scaled picture is kept.
    drop(decoded);
    Ok(Picture {
        width: size.0,
        height: size.1,
        layout: PictureLayout::Nv12,
        matrix: match colors.matrix {
            ffmpeg::ffi::SWS_CS_ITU709 => ColorMatrix::Bt709,
            ffmpeg::ffi::SWS_CS_BT2020 => ColorMatrix::Bt2020,
            _ => ColorMatrix::Bt601,
        },
        range: if colors.full_range {
            ColorRange::Full
        } else {
            ColorRange::Limited
        },
        luma,
        chroma,
    })
}

/// The first picture of an image file, with the stream's full size.
struct Decoded {
    frame: frame::Video,
    width: u32,
    height: u32,
    progressive: bool,
}

/// Decodes the first picture of the image at `path`. JPEG decodes at a half, a quarter or an
/// eighth of its size when that still covers `target` (the eighth when there is none); other
/// formats decode at full size.
fn decode_first(path: &Path, target: Option<(u32, u32)>) -> Result<Decoded, MediaError> {
    let open_error = |source| MediaError::Open {
        path: path.to_path_buf(),
        source,
    };
    let mut input = open_input(path)?;
    let stream = input
        .streams()
        .find(|stream| stream.parameters().medium() == ffmpeg::media::Type::Video)
        .ok_or_else(|| MediaError::NoVideo {
            path: path.to_path_buf(),
        })?;
    let index = stream.index();
    let parameters = stream.parameters();
    let fields = ffi::codec_fields(&parameters);
    let (width, height) = (
        u32::try_from(fields.width).unwrap_or(0),
        u32::try_from(fields.height).unwrap_or(0),
    );
    let context = ffmpeg::codec::Context::from_parameters(parameters).map_err(open_error)?;
    let id = context.id();
    let codec =
        ffmpeg::decoder::find(id).ok_or_else(|| open_error(ffmpeg::Error::DecoderNotFound))?;
    let lowres = match target {
        None => 3,
        Some(target) => (0..=3u32)
            .rev()
            .find(|shift| {
                width.div_ceil(1 << shift) >= target.0 && height.div_ceil(1 << shift) >= target.1
            })
            .unwrap_or(0),
    };
    let mut options = ffmpeg::Dictionary::new();
    // Decoders that cannot decode smaller ignore this.
    if id == ffmpeg::codec::Id::MJPEG && lowres > 0 {
        options.set("lowres", &lowres.to_string());
    }
    let mut decoder = context
        .decoder()
        .open_as_with(codec, options)
        .and_then(|opened| opened.video())
        .map_err(open_error)?;
    let mut frame = frame::Video::empty();
    let mut decoded = false;
    for (stream, packet) in input.packets() {
        if stream.index() != index {
            continue;
        }
        match decoder.send_packet(&packet) {
            // A damaged packet is skipped; another may hold the picture.
            Ok(()) | Err(ffmpeg::Error::InvalidData) => {}
            Err(source) => return Err(decode_error(path, source)),
        }
        if decoder.receive_frame(&mut frame).is_ok() {
            decoded = true;
            break;
        }
    }
    if !decoded {
        decoder
            .send_eof()
            .map_err(|source| decode_error(path, source))?;
        decoder
            .receive_frame(&mut frame)
            .map_err(|source| decode_error(path, source))?;
    }
    let progressive = id == ffmpeg::codec::Id::MJPEG
        && ffi::decoder_profile(&decoder) == ffmpeg::ffi::AV_PROFILE_MJPEG_HUFFMAN_PROGRESSIVE_DCT;
    // Containers that do not state the size have it on the decoded frame.
    let (width, height) = if width == 0 || height == 0 {
        (frame.width() << lowres, frame.height() << lowres)
    } else {
        (width, height)
    };
    Ok(Decoded {
        frame,
        width,
        height,
        progressive,
    })
}

/// How swscale reads `frame`'s colors, and how the picture made from it is tagged: an RGB
/// picture becomes full-range BT.709 YUV; a YUV one keeps its own matrix and range.
fn colors_of(frame: &frame::Video) -> ScaleColors {
    use ffmpeg::color::{Range, Space};
    use ffmpeg::ffi::{SWS_CS_BT2020, SWS_CS_ITU601, SWS_CS_ITU709};
    let format = frame.format();
    if ffi::is_rgb(format) {
        return ScaleColors {
            source_matrix: SWS_CS_ITU709,
            source_full_range: true,
            source_chroma: None,
            matrix: SWS_CS_ITU709,
            full_range: true,
        };
    }
    // JPEG's own formats are full range; so is anything tagged so.
    let jpeg_format = matches!(
        format,
        Pixel::YUVJ420P | Pixel::YUVJ422P | Pixel::YUVJ444P | Pixel::YUVJ440P | Pixel::YUVJ411P
    );
    let full_range = jpeg_format || frame.color_range() == Range::JPEG;
    let matrix = match frame.color_space() {
        Space::BT709 => SWS_CS_ITU709,
        Space::BT2020NCL | Space::BT2020CL => SWS_CS_BT2020,
        // JPEG's matrix (JFIF), and the default for the rest.
        _ => SWS_CS_ITU601,
    };
    let source_chroma = ffi::is_subsampled(format).then(|| {
        use ffmpeg::chroma::Location;
        match frame.chroma_location() {
            Location::Left => (0, 128),
            Location::Center => (128, 128),
            Location::TopLeft => (0, 0),
            Location::Top => (128, 0),
            Location::BottomLeft => (0, 256),
            Location::Bottom => (128, 256),
            // JFIF centers its chroma; video sites it left.
            Location::Unspecified if jpeg_format => (128, 128),
            Location::Unspecified => (0, 128),
        }
    });
    ScaleColors {
        source_matrix: matrix,
        source_full_range: full_range,
        source_chroma,
        matrix,
        full_range,
    }
}

fn decode_error(path: &Path, source: ffmpeg::Error) -> MediaError {
    MediaError::Decode {
        path: path.to_path_buf(),
        source,
    }
}
