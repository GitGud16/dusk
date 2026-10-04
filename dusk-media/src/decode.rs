//! Decoding video into pictures Dusk owns (docs/ARCHITECTURE.md, "Decoder pool" and "Memory
//! discipline"): hardware first (D3D11VA on Windows), software otherwise, and every frame
//! that leaves the decoder copied out of the decoder's own memory.

use std::path::{Path, PathBuf};

use dusk_core::{ColorMatrix, ColorRange, MediaTime, Picture, PictureLayout};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::frame;

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

/// A decoded frame: when it starts in the source and its picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedFrame {
    /// The frame's timestamp, from the start of the file.
    pub time: MediaTime,
    /// The picture, copied out of the decoder.
    pub picture: Picture,
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
}

/// Jumping further ahead than this, in microseconds, seeks instead of decoding every frame
/// in between.
const SEEK_AHEAD: i64 = 2_000_000;

impl VideoDecoder {
    /// Opens the first video stream of the local file at `path`. Cover art does not count as
    /// video.
    pub fn open(path: &Path, acceleration: Acceleration) -> Result<VideoDecoder, MediaError> {
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
            Acceleration::Hardware => open_decoder(&parameters, true)
                .or_else(|_| open_decoder(&parameters, false))
                .map_err(open_error)?,
            Acceleration::Software => open_decoder(&parameters, false).map_err(open_error)?,
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
            match &self.ahead {
                Some((next, _)) if *next > time => break,
                Some(_) => self.shown = self.ahead.take(),
                None if self.ended => break,
                None => self.ahead = self.decode_next()?,
            }
        }
        self.copy_shown()
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

/// Opens a decoder for `parameters`, on the GPU when `hardware` is set and possible.
fn open_decoder(
    parameters: &ffmpeg::codec::Parameters,
    hardware: bool,
) -> Result<ffmpeg::decoder::Video, ffmpeg::Error> {
    let mut context = ffmpeg::codec::Context::from_parameters(parameters.clone())?;
    let on_gpu = hardware && attach_hardware(&mut context);
    // Thread counts are set explicitly (docs/ARCHITECTURE.md, "Decoder pool").
    let count = if on_gpu {
        1
    } else {
        let cores = std::thread::available_parallelism().map_or(2, |cores| cores.get());
        (cores / 2).clamp(1, 4)
    };
    context.set_threading(ffmpeg::codec::threading::Config {
        kind: ffmpeg::codec::threading::Type::Frame,
        count,
    });
    context.decoder().video()
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
    let picture = Picture {
        width,
        height,
        layout,
        matrix: matrix_of(frame, width, height),
        range: range_of(frame),
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
