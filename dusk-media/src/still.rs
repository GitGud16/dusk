//! Still images (docs/ARCHITECTURE.md, "Decoder pool"): JPEG, PNG, WebP, TIFF and the other
//! pictures FFmpeg reads. A still is decoded once at import for its size and orientation,
//! then decoded again whenever a render needs it, scaled at once through swscale to the size
//! that render draws it at, and the decoded frame is released: only the scaled picture stays.

use std::path::Path;

use dusk_core::{ColorMatrix, ColorRange, Orientation, Picture, PictureLayout, YuvPicture};

use crate::decode::{primaries_of, transfer_of};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::frame;

use crate::ffi::{self, ScaleColors, ScaleSide, Scaler};
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
    probe.heif || probe.format == "image2" || probe.format.ends_with("_pipe")
}

/// Decodes the still image at `path` once and describes it.
pub fn still_info(path: &Path) -> Result<StillInfo, MediaError> {
    // The smallest size JPEG decodes at is enough to learn what the file holds.
    let decoded = decode_first(path, None, false)?;
    let orientation = decoded
        .display_matrix
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
    let decoded = decode_profiled(path, size)?;
    let (primaries, transfer) = (
        primaries_of(&decoded.frame),
        transfer_of(&decoded.frame, true),
    );
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
        primaries,
        transfer,
        peak_nits: 0,
        siting: crate::decode::siting_of(colors.siting),
        luma,
        chroma,
    })
}

/// Decodes the still image at `path` for dusq's CPU path, color profile and all, and
/// resamples it to 16-bit YUV 4:4:4 of `size` (width, height, as stored), as
/// [`VideoDecoder::next_normalized`](crate::VideoDecoder::next_normalized) does a video frame:
/// a YUV picture keeps its own matrix and range, an RGB one becomes full-range BT.709 YUV.
pub fn decode_still_normalized(path: &Path, size: (u32, u32)) -> Result<YuvPicture, MediaError> {
    let decoded = decode_profiled(path, size)?;
    let frame = &decoded.frame;
    let colors = colors_of(frame);
    let format = frame.format();
    let source = ScaleSide {
        format,
        size: (frame.width(), frame.height()),
        matrix: colors.source_matrix,
        full_range: colors.source_full_range,
        chroma: colors.source_chroma,
    };
    let destination = ScaleSide {
        format: ffi::YUV444P16,
        size,
        matrix: colors.matrix,
        full_range: colors.full_range,
        chroma: None,
    };
    let failed = |source| decode_error(path, source);
    let mut scaler = Scaler::new(source, destination).map_err(failed)?;
    let count = size.0 as usize * size.1 as usize;
    let mut planes = [0; 3].map(|_| vec![0u16; count]);
    {
        let [y, u, v] = &mut planes;
        scaler
            .frame_to_planes(
                frame,
                &mut [
                    ffi::samples_as_bytes(y),
                    ffi::samples_as_bytes(u),
                    ffi::samples_as_bytes(v),
                ],
            )
            .map_err(failed)?;
    }
    Ok(YuvPicture {
        width: size.0,
        height: size.1,
        planes,
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
        // RGB becomes YUV of its own depth; swscale shifts it up like any other.
        bits: u32::from(ffi::depth(format)),
        primaries: primaries_of(frame),
        transfer: transfer_of(frame, true),
        peak_nits: 0,
    })
}

/// The first picture of the image at `path`, decoded for a target of `size`, its primaries
/// read from its color profile when it has an RGB one.
fn decode_profiled(path: &Path, size: (u32, u32)) -> Result<Decoded, MediaError> {
    let decoded = decode_first(path, Some(size), false)?;
    // FFmpeg reads a profile's primaries only with its ICC support on, which refuses gray
    // and CMYK profiles; this one is RGB. If it fails anyway, the picture is sRGB.
    if ffi::has_rgb_icc_profile(&decoded.frame)
        && let Ok(tagged) = decode_first(path, Some(size), true)
    {
        return Ok(tagged);
    }
    Ok(decoded)
}

/// The first picture of an image file, with the stream's full size.
struct Decoded {
    frame: frame::Video,
    width: u32,
    height: u32,
    progressive: bool,
    /// How to turn and mirror it for display, when the file says.
    display_matrix: Option<[i32; 9]>,
}

/// Decodes the first picture of the image at `path`. JPEG decodes at a half, a quarter or an
/// eighth of its size when that still covers `target` (the eighth when there is none); other
/// formats decode at full size.
fn decode_first(
    path: &Path,
    target: Option<(u32, u32)>,
    icc_profiles: bool,
) -> Result<Decoded, MediaError> {
    let open_error = |source| MediaError::Open {
        path: path.to_path_buf(),
        source,
    };
    let mut input = open_input(path)?;
    if let Some(grid) = ffi::tile_grid(&input) {
        return decode_grid(path, input, grid);
    }
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
    if icc_profiles {
        options.set("flags2", "+icc_profiles");
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
    let display_matrix = ffi::frame_display_matrix(&frame);
    Ok(Decoded {
        frame,
        width,
        height,
        progressive,
        display_matrix,
    })
}

/// Decodes a picture stored as a grid of tiles (HEIC, AVIF; docs/ARCHITECTURE.md, "Decoder
/// pool"): each tile is decoded on its own and copied into place in the picture's window,
/// so only the picture and one tile are held at a time.
fn decode_grid(
    path: &Path,
    mut input: ffmpeg::format::context::Input,
    grid: ffi::TileGrid,
) -> Result<Decoded, MediaError> {
    let open_error = |source| MediaError::Open {
        path: path.to_path_buf(),
        source,
    };
    let (left, top, width, height) = grid.window;
    // The grid's profile, for FFmpeg's ICC support to read its colors from, as for JPEGs:
    // only an RGB one, which is what that support takes.
    let profile = grid
        .icc_profile
        .as_deref()
        .filter(|profile| ffi::is_rgb_profile(profile));
    let mut picture: Option<frame::Video> = None;
    for (stream, mut packet) in input.packets() {
        let Some(&(_, x, y)) = grid.tiles.iter().find(|tile| tile.0 == stream.index()) else {
            continue;
        };
        let context =
            ffmpeg::codec::Context::from_parameters(stream.parameters()).map_err(open_error)?;
        let codec = ffmpeg::decoder::find(context.id())
            .ok_or_else(|| open_error(ffmpeg::Error::DecoderNotFound))?;
        let mut options = ffmpeg::Dictionary::new();
        if let Some(profile) = profile {
            options.set("flags2", "+icc_profiles");
            ffi::attach_icc_profile(&mut packet, profile);
        }
        let mut decoder = context
            .decoder()
            .open_as_with(codec, options)
            .and_then(|opened| opened.video())
            .map_err(open_error)?;
        let mut tile = frame::Video::empty();
        decoder
            .send_packet(&packet)
            .and_then(|()| decoder.send_eof())
            .and_then(|()| decoder.receive_frame(&mut tile))
            .map_err(|source| decode_error(path, source))?;
        let picture = picture.get_or_insert_with(|| canvas(&tile, width, height));
        place(picture, &tile, x - left, y - top);
    }
    let frame = picture.ok_or_else(|| MediaError::NoVideo {
        path: path.to_path_buf(),
    })?;
    Ok(Decoded {
        frame,
        width,
        height,
        progressive: false,
        display_matrix: grid.display_matrix,
    })
}

/// A `width` by `height` frame in the format and colors of `tile`, for the tiles to go in.
fn canvas(tile: &frame::Video, width: u32, height: u32) -> frame::Video {
    let mut canvas = frame::Video::new(tile.format(), width, height);
    canvas.set_color_range(tile.color_range());
    canvas.set_color_space(tile.color_space());
    canvas.set_color_primaries(tile.color_primaries());
    canvas.set_color_transfer_characteristic(tile.color_transfer_characteristic());
    ffi::copy_chroma_location(tile, &mut canvas);
    canvas
}

/// Copies `tile` into `canvas` with its top-left corner at `x`, `y`, plane by plane, as much
/// of it as falls inside.
fn place(canvas: &mut frame::Video, tile: &frame::Video, x: i32, y: i32) {
    let bytes = ffi::sample_bytes(tile.format());
    let (shift_x, shift_y) = tile
        .format()
        .descriptor()
        .map_or((0, 0), |desc| (desc.log2_chroma_w(), desc.log2_chroma_h()));
    for plane in 0..tile.planes().min(canvas.planes()) {
        // The chroma planes are 1 and 2; luma and alpha are full size.
        let (sx, sy) = if plane == 1 || plane == 2 {
            (shift_x, shift_y)
        } else {
            (0, 0)
        };
        let (to_x, to_y) = (i64::from(x >> sx), i64::from(y >> sy));
        let tile_size = (tile.plane_width(plane), tile.plane_height(plane));
        let canvas_size = (canvas.plane_width(plane), canvas.plane_height(plane));
        // The tile's columns and rows that land on the canvas.
        let columns = (-to_x).max(0)..i64::from(tile_size.0).min(i64::from(canvas_size.0) - to_x);
        let rows = (-to_y).max(0)..i64::from(tile_size.1).min(i64::from(canvas_size.1) - to_y);
        if columns.is_empty() {
            continue;
        }
        let length = (columns.end - columns.start) as usize * bytes;
        let (tile_stride, canvas_stride) = (tile.stride(plane), canvas.stride(plane));
        for row in rows {
            let from = row as usize * tile_stride + columns.start as usize * bytes;
            let to =
                (to_y + row) as usize * canvas_stride + (to_x + columns.start) as usize * bytes;
            canvas.data_mut(plane)[to..to + length]
                .copy_from_slice(&tile.data(plane)[from..from + length]);
        }
    }
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
            siting: ffi::LEFT,
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
    // JFIF centers its chroma; video sites it left.
    let unspecified = if jpeg_format { (128, 128) } else { ffi::LEFT };
    let source_chroma =
        ffi::is_subsampled(format).then(|| crate::decode::chroma_siting(frame, unspecified));
    // Subsampled chroma stays where the source has it, so it is resampled only once.
    ScaleColors {
        source_matrix: matrix,
        source_full_range: full_range,
        source_chroma,
        matrix,
        full_range,
        siting: source_chroma.unwrap_or(ffi::LEFT),
    }
}

fn decode_error(path: &Path, source: ffmpeg::Error) -> MediaError {
    MediaError::Decode {
        path: path.to_path_buf(),
        source,
    }
}
