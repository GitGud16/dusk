//! Decoding video into pictures Dusk owns (docs/ARCHITECTURE.md, "Decoder pool" and "Memory
//! discipline"): hardware first (D3D11VA on Windows), software otherwise, and every frame
//! that leaves the decoder copied out of the decoder's own memory.

use std::path::{Path, PathBuf};

use dusk_core::color::{Primaries, Transfer, source_peak};
use dusk_core::{
    ChromaSiting, ColorMatrix, ColorRange, MediaTime, Picture, PictureLayout, YuvPicture,
};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::frame;

use crate::ffi::{ScaleSide, Scaler};
use crate::input::open_input;
use crate::{MediaError, ffi};

/// Whether a decoder may use the GPU's video decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acceleration {
    /// Software decoding, Dusk's default (docs/ARCHITECTURE.md, "Decoder pool").
    Software,
    /// The GPU's video decoder (D3D11VA on Windows) when the file and GPU allow it, else
    /// software. Not used by default: while frames are copied back to system memory it is no
    /// faster than software decoding and holds many times the memory.
    Hardware,
}

/// What comes after the frame a decoder returned last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Following {
    /// The next frame starts at this time, so the last one is shown until then.
    Next(MediaTime),
    /// The last frame was the stream's last; it is held from then on.
    End,
    /// Not decoded yet.
    Unknown,
}

/// How far [`VideoDecoder::step_to`] got.
#[derive(Debug)]
pub enum Step {
    /// On the way: one more frame was decoded.
    Working,
    /// There: the frame shown at the time asked for, or `None` before the stream's first
    /// frame, as [`VideoDecoder::frame_at`] returns it.
    Done(Option<DecodedFrame>),
}

/// A decoded frame: when it starts in the source and its picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedFrame {
    /// The frame's timestamp, from the start of the file.
    pub time: MediaTime,
    /// The picture, copied out of the decoder.
    pub picture: Picture,
}

/// A decoded frame normalized for dusq's CPU path: when it starts in the source, and its
/// picture as 16-bit YUV 4:4:4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedFrame {
    /// The frame's timestamp, from the start of the file.
    pub time: MediaTime,
    /// The picture, resampled out of the decoder.
    pub picture: YuvPicture,
}

/// Decodes the first video stream of a file, frame by frame or at any time.
pub struct VideoDecoder {
    path: PathBuf,
    input: ffmpeg::format::context::Input,
    stream: usize,
    time_base: ffmpeg::Rational,
    /// The container's start time, in microseconds; media time 0 is this moment.
    start: i64,
    decoder: ffmpeg::decoder::Video,
    /// The latest frame at or before the last requested time, still in the decoder's memory.
    shown: Option<(MediaTime, frame::Video)>,
    /// The frame after `shown`, already decoded.
    ahead: Option<(MediaTime, frame::Video)>,
    /// The demuxer reached the end of the file and the decoder was told so.
    drained: bool,
    /// The decoder has given out every frame.
    ended: bool,
    hardware: bool,
    /// What normalizes frames, kept from one frame to the next; boxed, since most decoders
    /// never normalize and the engine moves decoders around.
    scaler: Option<Box<Scaler>>,
}

/// Jumping further ahead than this, in microseconds, seeks instead of decoding every frame
/// in between.
const SEEK_AHEAD: i64 = 2_000_000;

impl VideoDecoder {
    /// Opens the first video stream of the local file at `path`. Cover art does not count as
    /// video.
    pub fn open(path: &Path, acceleration: Acceleration) -> Result<VideoDecoder, MediaError> {
        VideoDecoder::open_as(path, acceleration, None)
    }

    /// Opens the first video stream of `path` to decode in software on `threads` threads,
    /// as dusq does, whose thread rules differ from the app's (docs/REQUIREMENTS.md, "dusq").
    pub fn open_with_threads(path: &Path, threads: usize) -> Result<VideoDecoder, MediaError> {
        VideoDecoder::open_as(path, Acceleration::Software, Some(threads.max(1)))
    }

    fn open_as(
        path: &Path,
        acceleration: Acceleration,
        threads: Option<usize>,
    ) -> Result<VideoDecoder, MediaError> {
        let input = open_input(path)?;
        let open_error = |source| MediaError::Open {
            path: path.to_path_buf(),
            source,
        };
        let stream = input
            .streams()
            .find(|stream| {
                stream.parameters().medium() == ffmpeg::media::Type::Video
                    && !stream
                        .disposition()
                        .contains(ffmpeg::format::stream::Disposition::ATTACHED_PIC)
            })
            .ok_or_else(|| MediaError::NoVideo {
                path: path.to_path_buf(),
            })?;
        let (index, time_base) = (stream.index(), stream.time_base());
        let parameters = stream.parameters();
        let decoder = match acceleration {
            // A hardware decoder that fails to open falls back to software for this file.
            Acceleration::Hardware => open_decoder(&parameters, time_base, true, threads)
                .or_else(|_| open_decoder(&parameters, time_base, false, threads))
                .map_err(open_error)?,
            Acceleration::Software => {
                open_decoder(&parameters, time_base, false, threads).map_err(open_error)?
            }
        };
        let start = ffi::start_time(&input);
        Ok(VideoDecoder {
            path: path.to_path_buf(),
            input,
            stream: index,
            time_base,
            start,
            decoder,
            shown: None,
            ahead: None,
            drained: false,
            ended: false,
            hardware: false,
            scaler: None,
        })
    }

    /// Whether the last frame came from the hardware decoder.
    pub fn is_hardware(&self) -> bool {
        self.hardware
    }

    /// The frame shown at `time`: the last one whose timestamp is at or before `time`, held
    /// after the stream ends. `None` before the first frame.
    pub fn frame_at(&mut self, time: MediaTime) -> Result<Option<DecodedFrame>, MediaError> {
        let position = self.shown.as_ref().map(|(at, _)| *at);
        let behind = position.is_some_and(|at| time < at);
        let far_ahead = time.0 - position.map_or(0, |at| at.0) > SEEK_AHEAD;
        if behind || far_ahead {
            self.seek(time)?;
        }
        loop {
            if let Step::Done(frame) = self.step_to(time)? {
                return Ok(frame);
            }
        }
    }

    /// Goes on toward the frame shown at `time`, decoding at most one frame, so that getting
    /// there from a [`seek`](Self::seek) can be spread over several calls. Once there,
    /// [`following`](Self::following) knows when the next frame starts, as after
    /// [`frame_at`](Self::frame_at). It never seeks: the frame wanted must not be behind.
    pub fn step_to(&mut self, time: MediaTime) -> Result<Step, MediaError> {
        loop {
            match &self.ahead {
                Some((next, _)) if *next > time => break,
                Some(_) => self.shown = self.ahead.take(),
                None if self.ended => break,
                None => {
                    self.ahead = self.decode_next()?;
                    return Ok(Step::Working);
                }
            }
        }
        self.copy_shown().map(Step::Done)
    }

    /// What comes after the frame returned last: [`frame_at`](Self::frame_at) always knows,
    /// [`next_frame`](Self::next_frame) only at the end of the stream.
    pub fn following(&self) -> Following {
        match &self.ahead {
            Some((next, _)) => Following::Next(*next),
            None if self.ended => Following::End,
            None => Following::Unknown,
        }
    }

    /// The frame after the one last returned, in presentation order, or the first frame
    /// after a [`seek`](Self::seek); `None` after the last frame.
    pub fn next_frame(&mut self) -> Result<Option<DecodedFrame>, MediaError> {
        let next = match self.ahead.take() {
            Some(frame) => Some(frame),
            None if self.ended => None,
            None => self.decode_next()?,
        };
        if next.is_none() {
            return Ok(None);
        }
        self.shown = next;
        self.copy_shown()
    }

    /// The frame after the one last returned, as [`next_frame`](Self::next_frame) gives it,
    /// normalized at once for dusq's CPU path (docs/ARCHITECTURE.md, "Compress tool paths"):
    /// swscale resamples each plane to 16-bit YUV 4:4:4 of `size` with Catmull-Rom, reading
    /// the frame's chroma siting from its tags, and keeps its matrix and range, which the
    /// picture carries with its other tags (the defaults when untagged). The decoder's frame
    /// is let go before this returns.
    pub fn next_normalized(
        &mut self,
        size: (u32, u32),
    ) -> Result<Option<NormalizedFrame>, MediaError> {
        let next = match self.ahead.take() {
            Some(frame) => Some(frame),
            None if self.ended => None,
            None => self.decode_next()?,
        };
        self.shown = None;
        let Some((time, frame)) = next else {
            return Ok(None);
        };
        let picture = self.normalize(&frame, size)?;
        Ok(Some(NormalizedFrame { time, picture }))
    }

    /// `frame` as 16-bit YUV 4:4:4 of `size`, through the scaler kept for frames like it.
    fn normalize(
        &mut self,
        frame: &frame::Video,
        size: (u32, u32),
    ) -> Result<YuvPicture, MediaError> {
        let hardware = frame.format() == Pixel::D3D11;
        let system;
        let source = if hardware {
            system = ffi::transfer_to_system(frame)
                .map_err(|source| decode_error(&self.path, source))?;
            &system
        } else {
            frame
        };
        self.hardware = hardware;
        // The color tags come from the decoded frame; a transferred copy does not carry them.
        let format = source.format();
        let side = scale_side(frame, format, (source.width(), source.height()));
        let rgb = ffi::is_rgb(format);
        // swscale only resamples and shifts YUV up; RGB, which it turns into YUV inside
        // anyway, comes out as full-range BT.709 YUV.
        let (matrix, range) = if rgb {
            (ColorMatrix::Bt709, ColorRange::Full)
        } else {
            (
                matrix_of(frame, source.width(), source.height()),
                range_of(frame),
            )
        };
        let destination = ScaleSide {
            format: ffi::YUV444P16,
            size,
            matrix: if rgb {
                ffmpeg::ffi::SWS_CS_ITU709
            } else {
                side.matrix
            },
            full_range: range == ColorRange::Full,
            chroma: None,
        };
        let scaler = match self.scaler.take() {
            Some(scaler) if scaler.source() == side && scaler.destination() == destination => {
                scaler
            }
            _ => Box::new(
                Scaler::new(side, destination)
                    .map_err(|source| decode_error(&self.path, source))?,
            ),
        };
        let scaler = self.scaler.insert(scaler);
        let count = size.0 as usize * size.1 as usize;
        let mut planes = [0; 3].map(|_| vec![0u16; count]);
        {
            let [y, u, v] = &mut planes;
            scaler
                .frame_to_planes(
                    source,
                    &mut [
                        ffi::samples_as_bytes(y),
                        ffi::samples_as_bytes(u),
                        ffi::samples_as_bytes(v),
                    ],
                )
                .map_err(|source| decode_error(&self.path, source))?;
        }
        let transfer = transfer_of(frame, false);
        Ok(YuvPicture {
            width: size.0,
            height: size.1,
            planes,
            matrix,
            range,
            bits: u32::from(ffi::depth(format)),
            primaries: primaries_of(frame),
            transfer,
            peak_nits: peak_of(frame, transfer),
        })
    }

    /// The current frame, copied out of the decoder.
    fn copy_shown(&mut self) -> Result<Option<DecodedFrame>, MediaError> {
        let Some((at, frame)) = &self.shown else {
            return Ok(None);
        };
        let (picture, hardware) = picture_of(frame, &self.path)?;
        self.hardware = hardware;
        Ok(Some(DecodedFrame { time: *at, picture }))
    }

    /// Moves to the keyframe at or before `time` and forgets what was decoded; the next
    /// [`next_frame`](Self::next_frame) returns the frame there.
    pub fn seek(&mut self, time: MediaTime) -> Result<(), MediaError> {
        let target = time.0.max(0) + self.start;
        self.input
            .seek(target, ..target)
            .map_err(|source| decode_error(&self.path, source))?;
        self.decoder.flush();
        self.shown = None;
        self.ahead = None;
        self.drained = false;
        self.ended = false;
        Ok(())
    }

    /// The next frame from the decoder, reading packets as needed; `None` at the end.
    fn decode_next(&mut self) -> Result<Option<(MediaTime, frame::Video)>, MediaError> {
        let mut frame = frame::Video::empty();
        loop {
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => {
                    // A frame without any timestamp cannot be placed in time; skip it.
                    if let Some(timestamp) = frame.timestamp() {
                        return Ok(Some((self.media_time(timestamp), frame)));
                    }
                    continue;
                }
                Err(ffmpeg::Error::Eof) => {
                    self.ended = true;
                    return Ok(None);
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::util::error::EAGAIN => {
                    if self.drained {
                        self.ended = true;
                        return Ok(None);
                    }
                }
                Err(source) => return Err(decode_error(&self.path, source)),
            }
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() == self.stream => {
                    match self.decoder.send_packet(&packet) {
                        // A damaged packet is skipped; the decoder conceals what it lost.
                        Ok(()) | Err(ffmpeg::Error::InvalidData) => {}
                        Err(source) => return Err(decode_error(&self.path, source)),
                    }
                }
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => {
                    self.decoder
                        .send_eof()
                        .map_err(|source| decode_error(&self.path, source))?;
                    self.drained = true;
                }
                Err(source) => return Err(decode_error(&self.path, source)),
            }
        }
    }

    /// A timestamp in the stream's time base as media time, from the container's start.
    fn media_time(&self, timestamp: i64) -> MediaTime {
        let numerator = i128::from(timestamp) * 1_000_000 * i128::from(self.time_base.numerator());
        let denominator = i128::from(self.time_base.denominator()).max(1);
        // Round to the nearest microsecond, halves away from zero.
        let micros = (2 * numerator + numerator.signum() * denominator) / (2 * denominator);
        MediaTime(i64::try_from(micros).unwrap_or(i64::MAX) - self.start)
    }
}

/// Pictures with more pixels than this are above the 1080p class (docs/ARCHITECTURE.md,
/// "Decoder pool": the class ends at 2.1 Mpx).
pub const LARGE_FRAME: u64 = 2_100_000;

/// Pictures with more pixels than this, 6K and 8K video, decode on one thread: every further
/// thread holds frames of its own, which at that size adds up to more than the decoder budget
/// (docs/ARCHITECTURE.md, "Decoder pool").
pub const HUGE_FRAME: u64 = 9_000_000;

/// How many threads a software decoder of `width` by `height` pictures uses: half the cores,
/// at most 4, and one above [`HUGE_FRAME`].
fn software_threads(width: i32, height: i32) -> usize {
    let pixels = u64::try_from(width).unwrap_or(0) * u64::try_from(height).unwrap_or(0);
    if pixels > HUGE_FRAME {
        return 1;
    }
    let cores = std::thread::available_parallelism().map_or(2, |cores| cores.get());
    (cores / 2).clamp(1, 4)
}

/// Opens a decoder for `parameters`, whose packets come in `time_base`, on the GPU when
/// `hardware` is set and possible, and in software on `threads` threads when they are given.
fn open_decoder(
    parameters: &ffmpeg::codec::Parameters,
    time_base: ffmpeg::Rational,
    hardware: bool,
    threads: Option<usize>,
) -> Result<ffmpeg::decoder::Video, ffmpeg::Error> {
    let mut context = ffmpeg::codec::Context::from_parameters(parameters.clone())?;
    let on_gpu = hardware && attach_hardware(&mut context);
    // Thread counts are set explicitly (docs/ARCHITECTURE.md, "Decoder pool").
    let count = match (on_gpu, threads) {
        (true, _) => 1,
        (false, Some(threads)) => threads,
        (false, None) => {
            let fields = ffi::codec_fields(parameters);
            software_threads(fields.width, fields.height)
        }
    };
    context.set_threading(ffmpeg::codec::threading::Config {
        kind: ffmpeg::codec::threading::Type::Frame,
        count,
    });
    let mut decoder = context.decoder();
    // As for audio (see `AudioDecoder::open`), FFmpeg wants the packets' time base.
    decoder.set_packet_time_base(time_base);
    decoder.video()
}

/// Sets `context` up to decode on a D3D11VA device, if the codec and the machine allow it.
#[cfg(windows)]
fn attach_hardware(context: &mut ffmpeg::codec::Context) -> bool {
    let Some(codec) = ffmpeg::decoder::find(context.id()) else {
        return false;
    };
    if !ffi::supports_d3d11va(&codec) {
        return false;
    }
    let Some(device) = ffi::HwDevice::d3d11va() else {
        return false;
    };
    ffi::use_hw_device(context, &device);
    true
}

#[cfg(not(windows))]
fn attach_hardware(_context: &mut ffmpeg::codec::Context) -> bool {
    false
}

/// Copies `frame` into a [`Picture`]; also says whether it came from the hardware decoder.
fn picture_of(frame: &frame::Video, path: &Path) -> Result<(Picture, bool), MediaError> {
    let hardware = frame.format() == Pixel::D3D11;
    let system;
    let source = if hardware {
        system = ffi::transfer_to_system(frame).map_err(|source| decode_error(path, source))?;
        &system
    } else {
        frame
    };
    let (layout, planar) = match source.format() {
        Pixel::NV12 => (PictureLayout::Nv12, false),
        Pixel::P010LE => (PictureLayout::P010, false),
        Pixel::YUV420P | Pixel::YUVJ420P => (PictureLayout::Nv12, true),
        Pixel::YUV420P10LE => (PictureLayout::P010, true),
        other => {
            return Err(MediaError::UnsupportedPixelFormat {
                path: path.to_path_buf(),
                format: format!("{other:?}"),
            });
        }
    };
    let (width, height) = (source.width(), source.height());
    let bytes = layout.bytes_per_sample();
    let chroma_width = width.div_ceil(2) as usize * bytes;
    let chroma_height = height.div_ceil(2) as usize;
    let mut luma = copy_rows(
        source.data(0),
        source.stride(0),
        width as usize * bytes,
        height as usize,
    );
    let mut chroma = if planar {
        interleave(source, chroma_width, chroma_height, bytes)
    } else {
        copy_rows(
            source.data(1),
            source.stride(1),
            2 * chroma_width,
            chroma_height,
        )
    };
    if planar && layout == PictureLayout::P010 {
        // Planar 10-bit keeps the value in the low 10 bits; P010 keeps it in the top 10.
        to_top_bits(&mut luma);
        to_top_bits(&mut chroma);
    }
    // The color tags come from the decoded frame; a transferred copy does not carry them.
    let transfer = transfer_of(frame, false);
    let picture = Picture {
        width,
        height,
        layout,
        matrix: matrix_of(frame, width, height),
        range: range_of(frame),
        primaries: primaries_of(frame),
        transfer,
        peak_nits: peak_of(frame, transfer),
        siting: siting_of(chroma_siting(frame, ffi::LEFT)),
        luma,
        chroma,
    };
    Ok((picture, hardware))
}

/// `rows` rows of `row_bytes` bytes each, from a plane whose rows are `stride` bytes apart.
fn copy_rows(plane: &[u8], stride: usize, row_bytes: usize, rows: usize) -> Vec<u8> {
    let mut packed = Vec::with_capacity(row_bytes * rows);
    for row in plane.chunks(stride).take(rows) {
        packed.extend_from_slice(&row[..row_bytes]);
    }
    packed
}

/// The U and V planes of a planar frame as one plane of interleaved U, V samples.
fn interleave(frame: &frame::Video, row_bytes: usize, rows: usize, bytes: usize) -> Vec<u8> {
    let mut packed = Vec::with_capacity(2 * row_bytes * rows);
    let u_rows = frame.data(1).chunks(frame.stride(1));
    let v_rows = frame.data(2).chunks(frame.stride(2));
    for (u, v) in u_rows.zip(v_rows).take(rows) {
        let pairs = u[..row_bytes]
            .chunks(bytes)
            .zip(v[..row_bytes].chunks(bytes));
        for (u, v) in pairs {
            packed.extend_from_slice(u);
            packed.extend_from_slice(v);
        }
    }
    packed
}

/// Moves 10-bit values in 16-bit little-endian samples from the low bits to the top bits.
fn to_top_bits(samples: &mut [u8]) {
    let (samples, _) = samples.as_chunks_mut::<2>();
    for sample in samples {
        *sample = (u16::from_le_bytes(*sample) << 6).to_le_bytes();
    }
}

/// How swscale reads a decoded video frame of `format` and `size` (the frame itself, or its
/// copy out of the GPU) whose tags `frame` carries.
fn scale_side(frame: &frame::Video, format: Pixel, size: (u32, u32)) -> ScaleSide {
    use ffmpeg::ffi::{SWS_CS_BT2020, SWS_CS_ITU601, SWS_CS_ITU709};
    let matrix = match matrix_of(frame, size.0, size.1) {
        ColorMatrix::Bt601 => SWS_CS_ITU601,
        ColorMatrix::Bt709 => SWS_CS_ITU709,
        ColorMatrix::Bt2020 => SWS_CS_BT2020,
    };
    ScaleSide {
        format,
        size,
        matrix,
        full_range: ffi::is_rgb(format) || range_of(frame) == ColorRange::Full,
        // Untagged video sites its chroma left, as MPEG-2 and H.264 do by default.
        chroma: ffi::is_subsampled(format).then(|| chroma_siting(frame, ffi::LEFT)),
    }
}

/// A chroma position in 256ths of a luma pixel, as the picture keeps it.
pub(crate) fn siting_of((across, down): (i32, i32)) -> ChromaSiting {
    ChromaSiting { across, down }
}

/// Where `frame`'s subsampled chroma sits, in 256ths of a luma pixel: (across, down);
/// `unspecified` when the frame does not say.
pub(crate) fn chroma_siting(frame: &frame::Video, unspecified: (i32, i32)) -> (i32, i32) {
    use ffmpeg::chroma::Location;
    match frame.chroma_location() {
        Location::Left => (0, 128),
        Location::Center => (128, 128),
        Location::TopLeft => (0, 0),
        Location::Top => (128, 0),
        Location::BottomLeft => (0, 256),
        Location::Bottom => (128, 256),
        Location::Unspecified => unspecified,
    }
}

/// The frame's YUV matrix; untagged frames get the usual default for their size.
fn matrix_of(frame: &frame::Video, width: u32, height: u32) -> ColorMatrix {
    use ffmpeg::color::Space;
    match frame.color_space() {
        Space::BT709 => ColorMatrix::Bt709,
        Space::BT470BG | Space::SMPTE170M => ColorMatrix::Bt601,
        Space::BT2020NCL | Space::BT2020CL => ColorMatrix::Bt2020,
        // Untagged, or a matrix Dusk does not handle yet (docs/ARCHITECTURE.md, "Decoder
        // pool": BT.709 when width ≥ 1280 or height > 576, BT.601 otherwise).
        _ if width >= 1280 || height > 576 => ColorMatrix::Bt709,
        _ => ColorMatrix::Bt601,
    }
}

/// The frame's color primaries; untagged frames are taken as BT.709.
pub(crate) fn primaries_of(frame: &frame::Video) -> Primaries {
    use ffmpeg::color::Primaries as Tag;
    match frame.color_primaries() {
        Tag::BT470BG => Primaries::Bt601_625,
        Tag::SMPTE170M | Tag::SMPTE240M => Primaries::Bt601_525,
        Tag::BT2020 => Primaries::Bt2020,
        Tag::SMPTE432 => Primaries::DisplayP3,
        _ => Primaries::Bt709,
    }
}

/// The frame's transfer; an untagged photo is sRGB, untagged video BT.1886.
pub(crate) fn transfer_of(frame: &frame::Video, photo: bool) -> Transfer {
    use ffmpeg::color::TransferCharacteristic as Tag;
    match frame.color_transfer_characteristic() {
        Tag::SMPTE2084 => Transfer::Pq,
        Tag::ARIB_STD_B67 => Transfer::Hlg,
        Tag::IEC61966_2_1 => Transfer::Srgb,
        _ if photo => Transfer::Srgb,
        _ => Transfer::Bt1886,
    }
}

/// For an HDR frame, the peak tone mapping starts from, in nits; 0 for SDR.
fn peak_of(frame: &frame::Video, transfer: Transfer) -> u16 {
    if !transfer.is_hdr() {
        return 0;
    }
    let (max_cll, mastering_max) = ffi::light_levels(frame);
    // At most 10 000 nits, which fits.
    source_peak(max_cll, mastering_max).round() as u16
}

/// The frame's YUV range; untagged frames are limited range unless the format says full.
fn range_of(frame: &frame::Video) -> ColorRange {
    match frame.color_range() {
        ffmpeg::color::Range::JPEG => ColorRange::Full,
        ffmpeg::color::Range::MPEG => ColorRange::Limited,
        _ if frame.format() == Pixel::YUVJ420P => ColorRange::Full,
        _ => ColorRange::Limited,
    }
}

fn decode_error(path: &Path, source: ffmpeg::Error) -> MediaError {
    MediaError::Decode {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_above_9_megapixels_decode_on_one_thread() {
        assert_eq!(software_threads(7680, 4320), 1);
        assert_eq!(software_threads(6144, 3456), 1);
        assert!((1..=4).contains(&software_threads(3840, 2160)));
        assert!((1..=4).contains(&software_threads(1920, 1080)));
    }
}
