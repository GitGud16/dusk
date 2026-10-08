//! Writing export files (docs/ARCHITECTURE.md, "Export details"): video in MP4, MOV, MKV or
//! WebM from the first encoder of the chosen codec that opens, with AAC or Opus sound, and
//! sound alone as MP3, AAC, Opus or WAV. MP4, MOV and AAC files carry their index at the
//! front (`faststart`) so players can start before the whole file is read. All 0.1 exports
//! are 8-bit SDR BT.709.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use dusk_core::color::{Primaries, Transfer};
use dusk_core::{
    ChromaSiting, ColorMatrix, ColorRange, MediaTime, Picture, PictureLayout, SdrPicture,
};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::codec::capabilities::Capabilities;
use ffmpeg_next::format::sample::Type as SampleType;
use ffmpeg_next::format::{Pixel, Sample};
use ffmpeg_next::{ChannelLayout, Dictionary, Rational, codec, color, encoder, frame};

use crate::ffi::{ScaleSide, Scaler};
use crate::formats::{
    AudioCodec, AudioFormat, Container, ENCODERS, Encoder, Quality, VideoCodec, encoder_named,
    encoders_of,
};
use crate::input::init;
use crate::{MediaError, ffi};

/// The video an export writes.
#[derive(Clone, Copy, Debug)]
pub struct VideoSettings {
    /// The wanted width in pixels; the encoder may need it smaller (see [`Writer::size`]).
    pub width: u32,
    /// The wanted height in pixels.
    pub height: u32,
    /// Frames per second, as numerator and denominator.
    pub frame_rate: (u32, u32),
    pub codec: VideoCodec,
    pub quality: Quality,
    /// Only this encoder, instead of the codec's encoders in order.
    pub encoder: Option<&'static str>,
    /// How the frames are timed.
    pub timing: Timing,
}

/// How a video's frames are timed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Timing {
    /// One after another at the frame rate, as exports render them.
    #[default]
    Constant,
    /// At the times they are written with, as dusq passes a variable frame rate through; the
    /// frame rate is their average, which the encoder's rate control goes by.
    Source,
}

/// The time base of [`Timing::Source`]: MPEG's 90 kHz clock, which every container takes.
const SOURCE_CLOCK: i32 = 90_000;

impl VideoSettings {
    /// H.264 at the High preset from the first encoder that opens, as M1 exported.
    pub fn h264(width: u32, height: u32, frame_rate: (u32, u32)) -> VideoSettings {
        VideoSettings {
            width,
            height,
            frame_rate,
            codec: VideoCodec::H264,
            quality: Quality::HIGH,
            encoder: None,
            timing: Timing::Constant,
        }
    }
}

/// The sound an export writes, from interleaved stereo samples.
#[derive(Clone, Copy, Debug)]
pub struct AudioSettings {
    /// Samples a second.
    pub rate: u32,
    pub codec: AudioCodec,
    /// Bits a second; PCM ignores it.
    pub bit_rate: usize,
}

impl AudioSettings {
    /// `codec` for stereo at `rate`, at its usual bitrate: 192 kbit/s for AAC and MP3,
    /// 128 kbit/s for Opus.
    pub fn of(codec: AudioCodec, rate: u32) -> AudioSettings {
        let bit_rate = match codec {
            AudioCodec::Aac | AudioCodec::Mp3 => 192_000,
            AudioCodec::Opus => 128_000,
            AudioCodec::Pcm => 0,
        };
        AudioSettings {
            rate,
            codec,
            bit_rate,
        }
    }
}

/// A file being written: video with or without sound, or sound alone.
pub struct Writer {
    path: PathBuf,
    output: ffmpeg::format::context::Output,
    video: Option<Pictures>,
    audio: Option<Sound>,
}

/// The video track and its encoder.
struct Pictures {
    encoder: encoder::Video,
    track: Track,
    pixel: Pixel,
    size: (u32, u32),
    name: &'static str,
    frames: i64,
    timing: Timing,
    /// The last frame's timestamp, which the next must pass.
    last: Option<i64>,
    /// What turns upright SDR pictures into the encoder's YUV, once one is written.
    scaler: Option<Scaler>,
}

impl Pictures {
    /// The timestamp of the next frame, which starts at `time` in the source.
    fn next_pts(&mut self, time: MediaTime) -> i64 {
        let pts = match self.timing {
            Timing::Constant => self.frames,
            Timing::Source => {
                let ticks = i128::from(time.0) * i128::from(SOURCE_CLOCK);
                // To the nearest tick, halves away from zero.
                let rounded = (2 * ticks + ticks.signum() * 1_000_000) / 2_000_000;
                i64::try_from(rounded).unwrap_or(i64::MAX)
            }
        };
        // Encoders refuse a timestamp that does not move forward.
        let pts = self.last.map_or(pts, |last| pts.max(last + 1));
        self.last = Some(pts);
        self.frames += 1;
        pts
    }
}

/// The stream an encoder writes to.
struct Track {
    stream: usize,
    /// The encoder's time base and the stream's, which the muxer may have changed.
    encoder_base: Rational,
    stream_base: Rational,
    /// For video, how long a frame lasts at the average rate, in the encoder's time base:
    /// what a packet that comes without a duration is given, so the last frame of a file
    /// with source timing still counts.
    frame_duration: i64,
}

/// The sound track: samples wait until there is a whole encoder frame of them.
struct Sound {
    encoder: encoder::Audio,
    track: Track,
    rate: u32,
    format: Sample,
    frame_size: usize,
    /// The encoder takes only whole frames, so the last one is filled out with silence.
    pad_last: bool,
    name: &'static str,
    /// Interleaved stereo samples not encoded yet.
    pending: Vec<f32>,
    /// Samples per channel sent to the encoder so far.
    sent: i64,
}

impl Writer {
    /// Creates a `container` file at `path` (exports pass their `.part` name): video from
    /// the first of the codec's encoders that opens, and sound when `audio` is set.
    pub fn create(
        path: &Path,
        container: Container,
        video: VideoSettings,
        audio: Option<AudioSettings>,
    ) -> Result<Writer, MediaError> {
        if !container.video_codecs().contains(&video.codec) {
            return Err(MediaError::Unsupported {
                format: container.name(),
                codec: video.codec.name(),
            });
        }
        if let Some(audio) = audio
            && !container.audio_codecs().contains(&audio.codec)
        {
            return Err(MediaError::Unsupported {
                format: container.name(),
                codec: audio_name(audio.codec),
            });
        }
        init();
        let mut output = open_output(path, container.muxer())?;
        let global_header = global_header(&output);
        let candidates: Vec<&'static Encoder> = match video.encoder {
            Some(name) => encoder_named(name)
                .filter(|encoder| encoder.codec == video.codec)
                .into_iter()
                .collect(),
            None => encoders_of(video.codec).collect(),
        };
        let mut tried = Vec::new();
        let mut opened = None;
        for encoder in candidates {
            match open_video(encoder, &video, global_header) {
                Ok(found) => {
                    opened = Some((encoder, found));
                    break;
                }
                Err(error) => tried.push(format!("{}: {error}", encoder.name)),
            }
        }
        let Some((chosen, (video_encoder, size))) = opened else {
            return Err(MediaError::NoEncoder {
                codec: video.codec.name(),
                tried: tried.join("; "),
            });
        };
        let create_error = |source| MediaError::Create {
            path: path.to_path_buf(),
            source,
        };
        let video_stream = add_stream(&mut output, &video_encoder).map_err(create_error)?;
        if video.codec == VideoCodec::Hevc && matches!(container, Container::Mp4 | Container::Mov) {
            ffi::set_codec_tag(&mut output, video_stream, *b"hvc1");
        }
        let sound = match audio {
            Some(settings) => {
                Some(add_sound(&mut output, &settings, global_header).map_err(create_error)?)
            }
            None => None,
        };
        write_header(
            &mut output,
            matches!(container, Container::Mp4 | Container::Mov),
        )
        .map_err(create_error)?;
        let frame_duration = match video.timing {
            Timing::Constant => 1,
            Timing::Source => {
                let (num, den) = (
                    u64::from(video.frame_rate.0.max(1)),
                    u64::from(video.frame_rate.1),
                );
                ((SOURCE_CLOCK as u64 * den + num / 2) / num) as i64
            }
        };
        let pictures = Pictures {
            track: Track {
                encoder_base: video_encoder.time_base(),
                stream_base: stream_base(&output, video_stream),
                stream: video_stream,
                frame_duration,
            },
            encoder: video_encoder,
            pixel: chosen.pixel,
            size,
            name: chosen.name,
            frames: 0,
            timing: video.timing,
            last: None,
            scaler: None,
        };
        let audio = sound.map(|pending| pending.into_sound(&output));
        Ok(Writer {
            path: path.to_path_buf(),
            output,
            video: Some(pictures),
            audio,
        })
    }

    /// Creates a file of sound alone at `path` in `format`.
    pub fn sound(
        path: &Path,
        format: AudioFormat,
        audio: AudioSettings,
    ) -> Result<Writer, MediaError> {
        if audio.codec != format.codec() {
            return Err(MediaError::Unsupported {
                format: format.name(),
                codec: audio_name(audio.codec),
            });
        }
        init();
        let create_error = |source| MediaError::Create {
            path: path.to_path_buf(),
            source,
        };
        let mut output = open_output(path, format.muxer())?;
        let global_header = global_header(&output);
        let sound = add_sound(&mut output, &audio, global_header).map_err(create_error)?;
        write_header(&mut output, format == AudioFormat::M4a).map_err(create_error)?;
        let audio = sound.into_sound(&output);
        Ok(Writer {
            path: path.to_path_buf(),
            output,
            video: None,
            audio: Some(audio),
        })
    }

    /// The frame size the encoder takes: the wanted size, scaled down to the encoder's limits
    /// if needed (keeping its shape) and rounded down to the encoder's alignment; nothing for
    /// sound alone.
    pub fn size(&self) -> (u32, u32) {
        self.video.as_ref().map_or((0, 0), |video| video.size)
    }

    /// The encoder that writes the video, such as `h264_amf`; for sound alone, the sound's.
    pub fn encoder(&self) -> &str {
        match (&self.video, &self.audio) {
            (Some(video), _) => video.name,
            (None, Some(sound)) => sound.name,
            (None, None) => "",
        }
    }

    /// Encodes the next frame, an 8-bit NV12 picture of [`size`](Self::size) tagged
    /// limited-range BT.709.
    pub fn write_video(&mut self, picture: &Picture) -> Result<(), MediaError> {
        let Some(video) = self.video.as_mut() else {
            return Err(encode_error(&self.path, ffmpeg::Error::InvalidData));
        };
        if (picture.width, picture.height) != video.size || picture.layout != PictureLayout::Nv12 {
            return Err(encode_error(&self.path, ffmpeg::Error::InvalidData));
        }
        // A new frame each time: the encoder may still hold the previous one.
        let mut frame = frame::Video::new(video.pixel, video.size.0, video.size.1);
        fill(&mut frame, picture, video.pixel);
        frame.set_pts(Some(video.next_pts(MediaTime(0))));
        video
            .encoder
            .send_frame(&frame)
            .map_err(|source| encode_error(&self.path, source))?;
        self.drain_video()
    }

    /// Encodes the next frame from an upright SDR BT.709 picture of [`size`](Self::size),
    /// as dusq's CPU path makes them: swscale turns it into the encoder's limited-range YUV
    /// with Catmull-Rom, its chroma sited left. `time` is when the frame starts, which
    /// [`Timing::Source`] keeps; constant timing puts it one frame after the last.
    pub fn write_sdr(&mut self, picture: &SdrPicture, time: MediaTime) -> Result<(), MediaError> {
        let Some(video) = self.video.as_mut() else {
            return Err(encode_error(&self.path, ffmpeg::Error::InvalidData));
        };
        if (picture.width, picture.height) != video.size {
            return Err(encode_error(&self.path, ffmpeg::Error::InvalidData));
        }
        let scaler = match video.scaler.take() {
            Some(scaler) => scaler,
            None => {
                let (source, destination) = sdr_sides(video.size, video.pixel);
                Scaler::new(source, destination)
                    .map_err(|source| encode_error(&self.path, source))?
            }
        };
        let scaler = video.scaler.insert(scaler);
        let mut frame = frame::Video::new(video.pixel, video.size.0, video.size.1);
        // FFmpeg keeps planar RGB as green, blue, red.
        let [red, green, blue] = &picture.planes;
        scaler
            .planes_to_frame(&[green, blue, red], &mut frame)
            .map_err(|source| encode_error(&self.path, source))?;
        frame.set_pts(Some(video.next_pts(time)));
        video
            .encoder
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
        if let Some(sound) = self.audio.as_mut()
            && !sound.pending.is_empty()
        {
            let mut frames = sound.pending.len() / 2;
            if sound.pad_last {
                sound.pending.resize(2 * sound.frame_size, 0.0);
                frames = sound.frame_size;
            }
            self.encode_audio(frames)?;
        }
        if let Some(video) = self.video.as_mut() {
            video
                .encoder
                .send_eof()
                .map_err(|source| encode_error(&self.path, source))?;
        }
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
            .map_err(|source| encode_error(&self.path, source))
    }

    /// Sends the first `frames` pending stereo frames to the sound encoder.
    fn encode_audio(&mut self, frames: usize) -> Result<(), MediaError> {
        let Some(sound) = self.audio.as_mut() else {
            return Ok(());
        };
        let samples: Vec<f32> = sound.pending.drain(..2 * frames).collect();
        let mut frame = audio_frame(sound.format, &samples, sound.rate);
        frame.set_pts(Some(sound.sent));
        sound.sent += frames as i64;
        sound
            .encoder
            .send_frame(&frame)
            .map_err(|source| encode_error(&self.path, source))?;
        self.drain_audio()
    }

    fn drain_video(&mut self) -> Result<(), MediaError> {
        match self.video.as_mut() {
            Some(video) => drain(
                &mut video.encoder,
                &video.track,
                &mut self.output,
                &self.path,
            ),
            None => Ok(()),
        }
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
}

/// How an upright SDR picture of `size` turns into an encoder's `pixel` format: from planar
/// RGB to limited-range BT.709 YUV, chroma sited left (docs/ARCHITECTURE.md, "Compress tool
/// paths").
fn sdr_sides(size: (u32, u32), pixel: Pixel) -> (ScaleSide, ScaleSide) {
    let source = ScaleSide {
        format: Pixel::GBRP,
        size,
        matrix: ffmpeg::ffi::SWS_CS_ITU709,
        full_range: true,
        chroma: None,
    };
    let destination = ScaleSide {
        format: pixel,
        size,
        matrix: ffmpeg::ffi::SWS_CS_ITU709,
        full_range: false,
        chroma: Some(ffi::LEFT),
    };
    (source, destination)
}

/// `picture` as an encoder is handed it by [`Writer::write_sdr`], in NV12: what dusq's CPU
/// path writes, to hold against the GPU path's frames.
pub fn sdr_to_nv12(picture: &SdrPicture) -> Result<Picture, MediaError> {
    init();
    let size = (picture.width, picture.height);
    let failed = |source| MediaError::Encode {
        path: PathBuf::new(),
        source,
    };
    let (source, destination) = sdr_sides(size, Pixel::NV12);
    let mut scaler = Scaler::new(source, destination).map_err(failed)?;
    let mut frame = frame::Video::new(Pixel::NV12, size.0, size.1);
    let [red, green, blue] = &picture.planes;
    scaler
        .planes_to_frame(&[green, blue, red], &mut frame)
        .map_err(failed)?;
    let (width, height) = (size.0 as usize, size.1 as usize);
    let chroma_width = 2 * size.0.div_ceil(2) as usize;
    let rows = |plane: usize, row_bytes: usize, count: usize| -> Vec<u8> {
        frame
            .data(plane)
            .chunks(frame.stride(plane))
            .take(count)
            .flat_map(|row| row[..row_bytes].iter().copied())
            .collect()
    };
    Ok(Picture {
        width: size.0,
        height: size.1,
        layout: PictureLayout::Nv12,
        matrix: ColorMatrix::Bt709,
        range: ColorRange::Limited,
        primaries: Primaries::Bt709,
        transfer: Transfer::Bt1886,
        peak_nits: 0,
        siting: ChromaSiting::LEFT,
        luma: rows(0, width, height),
        chroma: rows(1, chroma_width, size.1.div_ceil(2) as usize),
    })
}

/// Opens the output file at `path` with `muxer`, only ever as a local file.
fn open_output(path: &Path, muxer: &str) -> Result<ffmpeg::format::context::Output, MediaError> {
    let path_str = path.to_str().ok_or_else(|| MediaError::NonUnicodePath {
        path: path.to_path_buf(),
    })?;
    let mut options = Dictionary::new();
    options.set("protocol_whitelist", "file");
    ffmpeg::format::output_as_with(path_str, muxer, options).map_err(|source| MediaError::Create {
        path: path.to_path_buf(),
        source,
    })
}

fn global_header(output: &ffmpeg::format::context::Output) -> bool {
    output
        .format()
        .flags()
        .contains(ffmpeg::format::Flags::GLOBAL_HEADER)
}

/// Writes the header; MPEG-4 files with their index at the front.
fn write_header(
    output: &mut ffmpeg::format::context::Output,
    faststart: bool,
) -> Result<(), ffmpeg::Error> {
    let mut header = Dictionary::new();
    if faststart {
        header.set("movflags", "+faststart");
    }
    output.write_header_with(header).map(|_| ())
}

fn stream_base(output: &ffmpeg::format::context::Output, index: usize) -> Rational {
    output
        .stream(index)
        .map_or(Rational::new(1, 90_000), |stream| stream.time_base())
}

/// A sound encoder with its stream, before the header is written.
struct NewSound {
    encoder: encoder::Audio,
    stream: usize,
    rate: u32,
    format: Sample,
    pad_last: bool,
    name: &'static str,
}

impl NewSound {
    fn into_sound(self, output: &ffmpeg::format::context::Output) -> Sound {
        // Encoders without a fixed frame size (PCM) take any number of samples.
        let frame_size = match self.encoder.frame_size() {
            0 => 1024,
            size => size as usize,
        };
        Sound {
            frame_size,
            track: Track {
                encoder_base: self.encoder.time_base(),
                stream_base: stream_base(output, self.stream),
                stream: self.stream,
                frame_duration: 0,
            },
            encoder: self.encoder,
            rate: self.rate,
            format: self.format,
            pad_last: self.pad_last,
            name: self.name,
            pending: Vec::new(),
            sent: 0,
        }
    }
}

/// Opens the sound encoder `settings` asks for and adds its stream to `output`.
fn add_sound(
    output: &mut ffmpeg::format::context::Output,
    settings: &AudioSettings,
    global_header: bool,
) -> Result<NewSound, ffmpeg::Error> {
    let (codec, name) = match settings.codec {
        AudioCodec::Aac => (encoder::find(codec::Id::AAC), "aac"),
        AudioCodec::Opus => (encoder::find_by_name("libopus"), "libopus"),
        AudioCodec::Mp3 => (encoder::find_by_name("libmp3lame"), "libmp3lame"),
        AudioCodec::Pcm => (encoder::find(codec::Id::PCM_S16LE), "pcm_s16le"),
    };
    let codec = codec.ok_or(ffmpeg::Error::EncoderNotFound)?;
    let format = sample_format(&codec)?;
    let fixed_frames = !codec
        .capabilities()
        .intersects(Capabilities::SMALL_LAST_FRAME | Capabilities::VARIABLE_FRAME_SIZE);
    let mut context = codec::Context::new_with_codec(codec).encoder().audio()?;
    let rate = i32::try_from(settings.rate).unwrap_or(48_000);
    context.set_rate(rate);
    context.set_channel_layout(ChannelLayout::STEREO);
    context.set_format(format);
    if settings.bit_rate > 0 && settings.codec != AudioCodec::Pcm {
        context.set_bit_rate(settings.bit_rate);
    }
    context.set_time_base(Rational::new(1, rate));
    if global_header {
        context.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let encoder = context.open_with(Dictionary::new())?;
    let stream = add_stream(output, &encoder)?;
    Ok(NewSound {
        pad_last: fixed_frames && encoder.frame_size() > 0,
        encoder,
        stream,
        rate: settings.rate,
        format,
        name,
    })
}

/// The sample layout to give `codec`, of those it takes: float when it can, else 16- or
/// 32-bit integers.
fn sample_format(codec: &ffmpeg::Codec) -> Result<Sample, ffmpeg::Error> {
    const PREFERRED: [Sample; 5] = [
        Sample::F32(SampleType::Planar),
        Sample::F32(SampleType::Packed),
        Sample::I16(SampleType::Packed),
        Sample::I16(SampleType::Planar),
        Sample::I32(SampleType::Planar),
    ];
    let supported: Vec<Sample> = codec
        .audio()?
        .formats()
        .map(|formats| formats.collect())
        .unwrap_or_default();
    PREFERRED
        .into_iter()
        .find(|format| supported.contains(format))
        .ok_or(ffmpeg::Error::InvalidData)
}

/// A frame of `stereo` (interleaved) samples in `format`.
fn audio_frame(format: Sample, stereo: &[f32], rate: u32) -> frame::Audio {
    let frames = stereo.len() / 2;
    let mut frame = frame::Audio::new(format, frames, ChannelLayout::STEREO);
    frame.set_rate(rate);
    let (pairs, _) = stereo.as_chunks::<2>();
    let to_i16 = |sample: f32| (sample.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
    let to_i32 =
        |sample: f32| (f64::from(sample.clamp(-1.0, 1.0)) * 2_147_483_647.0).round() as i32;
    match format {
        Sample::F32(SampleType::Planar) => {
            for channel in 0..2 {
                for (slot, pair) in frame.plane_mut::<f32>(channel).iter_mut().zip(pairs) {
                    *slot = pair[channel];
                }
            }
        }
        Sample::I16(SampleType::Planar) => {
            for channel in 0..2 {
                for (slot, pair) in frame.plane_mut::<i16>(channel).iter_mut().zip(pairs) {
                    *slot = to_i16(pair[channel]);
                }
            }
        }
        Sample::I32(SampleType::Planar) => {
            for channel in 0..2 {
                for (slot, pair) in frame.plane_mut::<i32>(channel).iter_mut().zip(pairs) {
                    *slot = to_i32(pair[channel]);
                }
            }
        }
        // Packed layouts keep both channels, interleaved, in the first plane.
        Sample::F32(SampleType::Packed) => {
            let (slots, _) = frame.data_mut(0).as_chunks_mut::<4>();
            for (slot, sample) in slots.iter_mut().zip(stereo) {
                *slot = sample.to_ne_bytes();
            }
        }
        Sample::I16(SampleType::Packed) => {
            let (slots, _) = frame.data_mut(0).as_chunks_mut::<2>();
            for (slot, sample) in slots.iter_mut().zip(stereo) {
                *slot = to_i16(*sample).to_ne_bytes();
            }
        }
        _ => {}
    }
    frame
}

fn audio_name(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Aac => "AAC",
        AudioCodec::Opus => "Opus",
        AudioCodec::Mp3 => "MP3",
        AudioCodec::Pcm => "PCM",
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
                if packet.duration() <= 0 && track.frame_duration > 0 {
                    packet.set_duration(track.frame_duration);
                }
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

/// The encoders that open on this machine, in the order they are tried. The first call opens
/// a real session of each at 640x480 (NVENC and AMF refuse smaller frames), which can take a
/// moment, so it belongs on a worker thread; later calls return the same list at once.
pub fn available_encoders() -> &'static [&'static Encoder] {
    static AVAILABLE: OnceLock<Vec<&'static Encoder>> = OnceLock::new();
    AVAILABLE.get_or_init(|| {
        init();
        let probe = VideoSettings::h264(640, 480, (30, 1));
        ENCODERS
            .iter()
            .filter(|encoder| {
                let video = VideoSettings {
                    codec: encoder.codec,
                    ..probe
                };
                open_video(encoder, &video, false).is_ok()
            })
            .collect()
    })
}

/// Opens `encoder` for `video`, at the largest size it takes.
fn open_video(
    encoder: &Encoder,
    video: &VideoSettings,
    global_header: bool,
) -> Result<(encoder::Video, (u32, u32)), ffmpeg::Error> {
    let codec = encoder::find_by_name(encoder.name).ok_or(ffmpeg::Error::EncoderNotFound)?;
    let (num, den) = video.frame_rate;
    let fps = f64::from(num) / f64::from(den.max(1));
    let size = encoder.fit((video.width, video.height), fps);
    let mut context = codec::Context::new_with_codec(codec).encoder().video()?;
    context.set_width(size.0);
    context.set_height(size.1);
    context.set_format(encoder.pixel);
    let (num, den) = (
        i32::try_from(num).unwrap_or(30),
        i32::try_from(den).unwrap_or(1),
    );
    context.set_time_base(match video.timing {
        Timing::Constant => Rational::new(den, num),
        Timing::Source => Rational::new(1, SOURCE_CLOCK),
    });
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
    let control = encoder.rate_control(video.quality, size, fps);
    context.set_bit_rate(control.bit_rate);
    if control.max_rate > 0 {
        context.set_max_bit_rate(control.max_rate);
    }
    if let Some(quality) = control.global_quality {
        context.set_global_quality(quality);
    }
    let mut options = Dictionary::new();
    if control.buffer > 0 {
        options.set("bufsize", &control.buffer.to_string());
    }
    for (key, value) in &control.options {
        options.set(key, value);
    }
    let encoder = context.open_with(options)?;
    Ok((encoder, size))
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
