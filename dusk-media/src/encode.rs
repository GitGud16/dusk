//! Writing MP4 files (docs/ARCHITECTURE.md, "Export details"): H.264 from the first encoder in
//! the order that opens, AAC sound, and the index at the front (`faststart`) so players can
//! start before the whole file is read. All 0.1 exports are 8-bit SDR BT.709.

use std::path::{Path, PathBuf};

use dusk_core::{Picture, PictureLayout};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::format::sample::Type as SampleType;
use ffmpeg_next::format::{Pixel, Sample};
use ffmpeg_next::{ChannelLayout, Dictionary, Rational, codec, color, encoder, frame};

use crate::MediaError;
use crate::input::init;

/// The video an export writes.
#[derive(Clone, Copy, Debug)]
pub struct VideoSettings {
    /// The wanted width in pixels; the encoder may need it smaller (see [`Mp4Writer::size`]).
    pub width: u32,
    /// The wanted height in pixels.
    pub height: u32,
    /// Frames per second, as numerator and denominator.
    pub frame_rate: (u32, u32),
}

/// The sound an export writes: interleaved stereo at `rate` samples per second.
#[derive(Clone, Copy, Debug)]
pub struct AudioSettings {
    /// Samples per second.
    pub rate: u32,
}

/// What an encoder accepts (docs/ARCHITECTURE.md, "Encoder constraints"). FFmpeg cannot
/// report these, so they are fixed, conservative values; an encoder that refuses to open
/// anyway is skipped for the next one.
struct Limits {
    name: &'static str,
    pixel: Pixel,
    max_side: u32,
    /// Pixels a frame: 9.4 Mpx at H.264 level 5.2.
    max_area: u64,
    /// Luma samples a second at level 5.2.
    max_luma_rate: u64,
    alignment: u32,
}

/// The H.264 encoders in the order they are tried (CLAUDE.md, "Stack"). Hardware H.264
/// encoders cap each side at 4096; AMD's older cards cap at 4096x2160, which the area covers.
const H264: [Limits; 4] = [
    Limits {
        name: "h264_nvenc",
        pixel: Pixel::NV12,
        max_side: 4096,
        max_area: 9_437_184,
        max_luma_rate: 530_841_600,
        alignment: 2,
    },
    Limits {
        name: "h264_qsv",
        pixel: Pixel::NV12,
        max_side: 4096,
        max_area: 9_437_184,
        max_luma_rate: 530_841_600,
        alignment: 2,
    },
    Limits {
        name: "h264_amf",
        pixel: Pixel::NV12,
        max_side: 4096,
        max_area: 8_847_360,
        max_luma_rate: 530_841_600,
        alignment: 2,
    },
    Limits {
        name: "libopenh264",
        pixel: Pixel::YUV420P,
        max_side: 4096,
        max_area: 9_437_184,
        max_luma_rate: 530_841_600,
        alignment: 2,
    },
];

/// AAC at this rate, for stereo.
const AUDIO_BIT_RATE: usize = 192_000;

/// An MP4 file being written: H.264 video and, if asked for, AAC sound.
pub struct Mp4Writer {
    path: PathBuf,
    output: ffmpeg::format::context::Output,
    video_encoder: encoder::Video,
    video: Track,
    pixel: Pixel,
    size: (u32, u32),
    name: &'static str,
    frames: i64,
    audio: Option<Sound>,
}

/// The stream an encoder writes to.
struct Track {
    stream: usize,
    /// The encoder's time base and the stream's, which the muxer may have changed.
    encoder_base: Rational,
    stream_base: Rational,
}

/// The sound track: samples wait until there is a whole encoder frame of them.
struct Sound {
    encoder: encoder::Audio,
    track: Track,
    rate: u32,
    frame_size: usize,
    /// Interleaved stereo samples not encoded yet.
    pending: Vec<f32>,
    /// Samples per channel sent to the encoder so far.
    sent: i64,
}

impl Mp4Writer {
    /// Creates the MP4 file at `path` (exports pass their `.part` name): H.264 from the
    /// first encoder in the order that opens, and AAC when `audio` is set.
    pub fn create(
        path: &Path,
        video: VideoSettings,
        audio: Option<AudioSettings>,
    ) -> Result<Mp4Writer, MediaError> {
        init();
        let path_str = path.to_str().ok_or_else(|| MediaError::NonUnicodePath {
            path: path.to_path_buf(),
        })?;
        let create_error = |source| MediaError::Create {
            path: path.to_path_buf(),
            source,
        };
        // Only ever a local file, whatever the path looks like.
        let mut options = Dictionary::new();
        options.set("protocol_whitelist", "file");
        let mut output =
            ffmpeg::format::output_as_with(path_str, "mp4", options).map_err(create_error)?;
        let global_header = output
            .format()
            .flags()
            .contains(ffmpeg::format::Flags::GLOBAL_HEADER);

        let mut tried = Vec::new();
        let mut opened = None;
        for limits in &H264 {
            match open_video(limits, &video, global_header) {
                Ok(found) => {
                    opened = Some((limits, found));
                    break;
                }
                Err(error) => tried.push(format!("{}: {error}", limits.name)),
            }
        }
        let Some((limits, (encoder, size))) = opened else {
            return Err(MediaError::NoEncoder {
                tried: tried.join("; "),
            });
        };
        let video_stream = add_stream(&mut output, &encoder).map_err(create_error)?;
        let sound = match audio {
            Some(settings) => {
                let encoder = open_audio(settings.rate, global_header).map_err(create_error)?;
                let stream = add_stream(&mut output, &encoder).map_err(create_error)?;
                Some((encoder, stream, settings.rate))
            }
            None => None,
        };

        let mut header = Dictionary::new();
        header.set("movflags", "+faststart");
        output.write_header_with(header).map_err(create_error)?;

        let stream_base = |output: &ffmpeg::format::context::Output, index| {
            output
                .stream(index)
                .map_or(Rational::new(1, 90_000), |stream| stream.time_base())
        };
        let video = Track {
            encoder_base: encoder.time_base(),
            stream_base: stream_base(&output, video_stream),
            stream: video_stream,
        };
        let audio = sound.map(|(encoder, stream, rate)| Sound {
            frame_size: (encoder.frame_size() as usize).max(1),
            track: Track {
                encoder_base: encoder.time_base(),
                stream_base: stream_base(&output, stream),
                stream,
            },
            encoder,
            rate,
            pending: Vec::new(),
            sent: 0,
        });
        Ok(Mp4Writer {
            path: path.to_path_buf(),
            output,
            video_encoder: encoder,
            video,
            pixel: limits.pixel,
            size,
            name: limits.name,
            frames: 0,
            audio,
        })
    }

    /// The frame size the encoder takes: the wanted size, scaled down to the encoder's limits
    /// if needed (keeping its shape) and rounded down to the encoder's alignment.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The encoder that was opened, such as `h264_amf`.
    pub fn encoder(&self) -> &str {
        self.name
    }

    /// Encodes the next frame, an 8-bit picture of [`size`](Self::size) tagged limited-range
    /// BT.709.
    pub fn write_video(&mut self, picture: &Picture) -> Result<(), MediaError> {
        if (picture.width, picture.height) != self.size || picture.layout != PictureLayout::Nv12 {
            return Err(self.encode_error(ffmpeg::Error::InvalidData));
        }
        // A new frame each time: the encoder may still hold the previous one.
        let mut frame = frame::Video::new(self.pixel, self.size.0, self.size.1);
        fill(&mut frame, picture, self.pixel);
        frame.set_pts(Some(self.frames));
        self.frames += 1;
        self.video_encoder
            .send_frame(&frame)
            .map_err(|source| encode_error(&self.path, source))?;
        self.drain_video()
    }

    /// Encodes the next interleaved stereo samples; ignored without sound.
    pub fn write_audio(&mut self, stereo: &[f32]) -> Result<(), MediaError> {
        let Some(sound) = self.audio.as_mut() else {
            return Ok(());
        };
        sound.pending.extend_from_slice(stereo);
        let frame_size = sound.frame_size;
        while self
            .audio
            .as_ref()
            .is_some_and(|sound| sound.pending.len() >= 2 * frame_size)
        {
            self.encode_audio(frame_size)?;
        }
        Ok(())
    }

    /// Encodes what the encoders still hold and finishes the file.
    pub fn finish(mut self) -> Result<(), MediaError> {
        if let Some(sound) = &self.audio
            && !sound.pending.is_empty()
        {
            // The AAC encoder takes a short last frame.
            let frames = sound.pending.len() / 2;
            self.encode_audio(frames)?;
        }
        self.video_encoder
            .send_eof()
            .map_err(|source| encode_error(&self.path, source))?;
        self.drain_video()?;
        if let Some(sound) = self.audio.as_mut() {
            sound
                .encoder
                .send_eof()
                .map_err(|source| encode_error(&self.path, source))?;
        }
        self.drain_audio()?;
        self.output
            .write_trailer()
            .map_err(|source| self.encode_error(source))
    }

    /// Sends the first `frames` pending stereo frames to the AAC encoder.
    fn encode_audio(&mut self, frames: usize) -> Result<(), MediaError> {
        let Some(sound) = self.audio.as_mut() else {
            return Ok(());
        };
        let mut frame = frame::Audio::new(
            Sample::F32(SampleType::Planar),
            frames,
            ChannelLayout::STEREO,
        );
        frame.set_rate(sound.rate);
        let samples: Vec<f32> = sound.pending.drain(..2 * frames).collect();
        let (pairs, _) = samples.as_chunks::<2>();
        for channel in 0..2 {
            let plane = frame.plane_mut::<f32>(channel);
            for (slot, pair) in plane.iter_mut().zip(pairs) {
                *slot = pair[channel];
            }
        }
        frame.set_pts(Some(sound.sent));
        sound.sent += frames as i64;
        sound
            .encoder
            .send_frame(&frame)
            .map_err(|source| encode_error(&self.path, source))?;
        self.drain_audio()
    }

    fn drain_video(&mut self) -> Result<(), MediaError> {
        drain(
            &mut self.video_encoder,
            &self.video,
            &mut self.output,
            &self.path,
        )
    }

    fn drain_audio(&mut self) -> Result<(), MediaError> {
        match self.audio.as_mut() {
            Some(sound) => drain(
                &mut sound.encoder,
                &sound.track,
                &mut self.output,
                &self.path,
            ),
            None => Ok(()),
        }
    }

    fn encode_error(&self, source: ffmpeg::Error) -> MediaError {
        encode_error(&self.path, source)
    }
}

/// Writes every packet `encoder` has ready to its stream.
fn drain(
    encoder: &mut encoder::Encoder,
    track: &Track,
    output: &mut ffmpeg::format::context::Output,
    path: &Path,
) -> Result<(), MediaError> {
    let mut packet = ffmpeg::Packet::empty();
    loop {
        match encoder.receive_packet(&mut packet) {
            Ok(()) => {
                packet.set_stream(track.stream);
                packet.rescale_ts(track.encoder_base, track.stream_base);
                packet
                    .write_interleaved(output)
                    .map_err(|source| encode_error(path, source))?;
            }
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::util::error::EAGAIN => {
                return Ok(());
            }
            Err(source) => return Err(encode_error(path, source)),
        }
    }
}

/// Opens the encoder `limits` describes for `video`, at the largest size it takes.
fn open_video(
    limits: &Limits,
    video: &VideoSettings,
    global_header: bool,
) -> Result<(encoder::Video, (u32, u32)), ffmpeg::Error> {
    let codec = encoder::find_by_name(limits.name).ok_or(ffmpeg::Error::EncoderNotFound)?;
    let (num, den) = video.frame_rate;
    let fps = f64::from(num) / f64::from(den.max(1));
    let size = fit(limits, (video.width, video.height), fps);
    let mut context = codec::Context::new_with_codec(codec).encoder().video()?;
    context.set_width(size.0);
    context.set_height(size.1);
    context.set_format(limits.pixel);
    let (num, den) = (
        i32::try_from(num).unwrap_or(30),
        i32::try_from(den).unwrap_or(1),
    );
    context.set_time_base(Rational::new(den, num));
    context.set_frame_rate(Some(Rational::new(num, den)));
    // A keyframe every two seconds keeps seeking in the file quick.
    context.set_gop((2.0 * fps).round().max(1.0) as u32);
    context.set_colorspace(color::Space::BT709);
    context.set_color_range(color::Range::MPEG);
    context.set_color_primaries(color::Primaries::BT709);
    context.set_color_transfer_characteristic(color::TransferCharacteristic::BT709);
    if global_header {
        context.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    // The "High" quality preset (80 on the 0-100 slider of M4), in each encoder's own terms.
    let mut options = Dictionary::new();
    match limits.name {
        "h264_nvenc" => {
            options.set("preset", "p5");
            options.set("rc", "vbr");
            options.set("cq", "21");
            context.set_bit_rate(0);
        }
        "h264_qsv" => {
            options.set("preset", "medium");
            context.set_global_quality(21);
        }
        "h264_amf" => {
            options.set("usage", "transcoding");
            options.set("quality", "quality");
            options.set("rc", "cqp");
            options.set("qp_i", "20");
            options.set("qp_p", "22");
            options.set("qp_b", "24");
        }
        _ => {
            // OpenH264 has no constant-quality mode: 0.15 bits a pixel.
            let bits = 0.15 * f64::from(size.0) * f64::from(size.1) * fps;
            context.set_bit_rate(bits as usize);
            options.set("rc_mode", "bitrate");
            options.set("allow_skip_frames", "0");
        }
    }
    let encoder = context.open_with(options)?;
    Ok((encoder, size))
}

/// Opens the AAC encoder for stereo at `rate`.
fn open_audio(rate: u32, global_header: bool) -> Result<encoder::Audio, ffmpeg::Error> {
    let codec = encoder::find(codec::Id::AAC).ok_or(ffmpeg::Error::EncoderNotFound)?;
    let mut context = codec::Context::new_with_codec(codec).encoder().audio()?;
    let rate = i32::try_from(rate).unwrap_or(48_000);
    context.set_rate(rate);
    context.set_channel_layout(ChannelLayout::STEREO);
    context.set_format(Sample::F32(SampleType::Planar));
    context.set_bit_rate(AUDIO_BIT_RATE);
    context.set_time_base(Rational::new(1, rate));
    if global_header {
        context.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    context.open_with(Dictionary::new())
}

/// Adds a stream for `encoder` and returns its index.
fn add_stream<E>(
    output: &mut ffmpeg::format::context::Output,
    encoder: &E,
) -> Result<usize, ffmpeg::Error>
where
    E: AsRef<codec::Context>,
{
    let codec = encoder.as_ref().codec();
    let mut stream = output.add_stream(codec)?;
    stream.set_parameters(encoder);
    Ok(stream.index())
}

/// The largest size within `limits` with the shape of `wanted`, rounded down to the
/// encoder's alignment.
fn fit(limits: &Limits, wanted: (u32, u32), fps: f64) -> (u32, u32) {
    let (width, height) = (f64::from(wanted.0.max(1)), f64::from(wanted.1.max(1)));
    let side = f64::from(limits.max_side);
    let area = (limits.max_area as f64).min(limits.max_luma_rate as f64 / fps.max(1.0));
    let scale = (side / width)
        .min(side / height)
        .min((area / (width * height)).sqrt())
        .min(1.0);
    let align = |length: f64| {
        let length = (length * scale).floor() as u32;
        (length / limits.alignment * limits.alignment).max(limits.alignment)
    };
    (align(width), align(height))
}

/// Copies an NV12 picture into an encoder frame of `pixel` format (NV12 or planar YUV420P).
fn fill(frame: &mut frame::Video, picture: &Picture, pixel: Pixel) {
    let width = picture.width as usize;
    let (chroma_width, chroma_height) = picture.chroma_size();
    let (chroma_width, chroma_height) = (chroma_width as usize, chroma_height as usize);
    let stride = frame.stride(0);
    for (row, source) in picture.luma.chunks(width).enumerate() {
        frame.data_mut(0)[row * stride..row * stride + width].copy_from_slice(source);
    }
    let rows = picture.chroma.chunks(2 * chroma_width).take(chroma_height);
    if pixel == Pixel::NV12 {
        let stride = frame.stride(1);
        for (row, source) in rows.enumerate() {
            frame.data_mut(1)[row * stride..row * stride + 2 * chroma_width]
                .copy_from_slice(source);
        }
    } else {
        let (u_stride, v_stride) = (frame.stride(1), frame.stride(2));
        for (row, source) in rows.enumerate() {
            let (pairs, _) = source.as_chunks::<2>();
            for (x, [u, v]) in pairs.iter().enumerate() {
                frame.data_mut(1)[row * u_stride + x] = *u;
                frame.data_mut(2)[row * v_stride + x] = *v;
            }
        }
    }
}

fn encode_error(path: &Path, source: ffmpeg::Error) -> MediaError {
    MediaError::Encode {
        path: path.to_path_buf(),
        source,
    }
}
