//! Decoding audio into interleaved 32-bit float samples at the rate and channel count the
//! caller plays or encodes at.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use dusk_core::MediaTime;
use ffmpeg_next as ffmpeg;
use ffmpeg_next::format::Sample;
use ffmpeg_next::format::sample::Type as SampleType;
use ffmpeg_next::software::resampling;
use ffmpeg_next::{ChannelLayout, frame};

use crate::input::open_input;
use crate::{MediaError, ffi};

/// Decodes the first audio stream of a file to interleaved `f32` samples, resampled to a fixed
/// rate and channel count.
pub struct AudioDecoder {
    path: PathBuf,
    input: ffmpeg::format::context::Input,
    stream: usize,
    time_base: ffmpeg::Rational,
    /// The container's start time, in microseconds; media time 0 is this moment.
    start: i64,
    decoder: ffmpeg::decoder::Audio,
    rate: u32,
    channels: u16,
    /// Converts decoded frames to the output format; made from the first frame after opening
    /// or seeking, and again if the decoded format changes.
    resampler: Option<Resampler>,
    /// Converted samples not read yet, interleaved.
    pending: VecDeque<f32>,
    /// The index (at the output rate, counted from media time 0) of the next converted
    /// frame; set by the first decoded frame after opening or seeking.
    position: Option<i64>,
    /// Frames before this index are dropped, so reading starts exactly where a seek asked.
    skip_until: i64,
    /// The demuxer reached the end of the file and the decoder was told so.
    drained: bool,
    /// Every sample has been converted.
    ended: bool,
}

/// A resampler and the decoded format it was made for.
struct Resampler {
    context: resampling::Context,
    input: (Sample, u32, i32),
}

impl AudioDecoder {
    /// Opens the first audio stream of the local file at `path`, to be read at `rate` samples
    /// per second with `channels` channels.
    pub fn open(path: &Path, rate: u32, channels: u16) -> Result<AudioDecoder, MediaError> {
        let input = open_input(path)?;
        let open_error = |source| MediaError::Open {
            path: path.to_path_buf(),
            source,
        };
        let stream = input
            .streams()
            .find(|stream| stream.parameters().medium() == ffmpeg::media::Type::Audio)
            .ok_or_else(|| MediaError::NoAudio {
                path: path.to_path_buf(),
            })?;
        let (index, time_base) = (stream.index(), stream.time_base());
        let mut context =
            ffmpeg::codec::Context::from_parameters(stream.parameters()).map_err(open_error)?;
        // Audio decoding is light; one thread is plenty.
        context.set_threading(ffmpeg::codec::threading::Config {
            kind: ffmpeg::codec::threading::Type::Frame,
            count: 1,
        });
        let decoder = context.decoder().audio().map_err(open_error)?;
        let start = ffi::start_time(&input);
        Ok(AudioDecoder {
            path: path.to_path_buf(),
            input,
            stream: index,
            time_base,
            start,
            decoder,
            rate: rate.max(1),
            channels: channels.max(1),
            resampler: None,
            pending: VecDeque::new(),
            position: None,
            // Encoder priming before media time 0 is never played.
            skip_until: 0,
            drained: false,
            ended: false,
        })
    }

    /// Moves so that the next sample read is the one at `time`.
    pub fn seek(&mut self, time: MediaTime) -> Result<(), MediaError> {
        let time = time.0.max(0);
        let target = time + self.start;
        self.input
            .seek(target, ..target)
            .map_err(|source| decode_error(&self.path, source))?;
        self.decoder.flush();
        self.resampler = None;
        self.pending.clear();
        self.position = None;
        self.skip_until = self.frame_index(time);
        self.drained = false;
        self.ended = false;
        Ok(())
    }

    /// Fills `out` with the next samples, interleaved, and returns how many frames (samples
    /// per channel) it wrote: fewer than fit only at the end of the stream, 0 after it.
    pub fn read(&mut self, out: &mut [f32]) -> Result<usize, MediaError> {
        let channels = usize::from(self.channels);
        let wanted = out.len() / channels;
        while self.pending.len() < wanted * channels && !self.ended {
            self.decode_more()?;
        }
        let frames = (self.pending.len() / channels).min(wanted);
        for (slot, sample) in out.iter_mut().zip(self.pending.drain(..frames * channels)) {
            *slot = sample;
        }
        Ok(frames)
    }

    /// Decodes and converts one more frame, reading packets as needed.
    fn decode_more(&mut self) -> Result<(), MediaError> {
        let mut decoded = frame::Audio::empty();
        loop {
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => return self.convert(&mut decoded),
                Err(ffmpeg::Error::Eof) => {
                    self.flush_resampler()?;
                    self.ended = true;
                    return Ok(());
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::util::error::EAGAIN => {
                    if self.drained {
                        self.flush_resampler()?;
                        self.ended = true;
                        return Ok(());
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

    /// Converts a decoded frame to the output format and keeps the samples.
    fn convert(&mut self, decoded: &mut frame::Audio) -> Result<(), MediaError> {
        // PCM in WAV, among others, leaves the channel order unspecified. The resampler takes
        // that as the default layout for the channel count, then refuses every frame still
        // marked unspecified as a changed input, so the frame is given that layout too. An
        // unspecified layout owns no memory, so replacing it frees nothing.
        if decoded.channel_layout().is_empty() {
            decoded.set_channel_layout(ChannelLayout::default(i32::from(decoded.channels())));
        }
        if self.position.is_none() {
            let start = decoded.timestamp().map_or(0, |ts| self.micros(ts));
            self.position = Some(self.frame_index(start));
        }
        let key = (
            decoded.format(),
            decoded.rate(),
            decoded.channel_layout().channels(),
        );
        if self.resampler.as_ref().is_none_or(|r| r.input != key) {
            let context = resampling::Context::get(
                decoded.format(),
                decoded.channel_layout(),
                decoded.rate(),
                Sample::F32(SampleType::Packed),
                ChannelLayout::default(i32::from(self.channels)),
                self.rate,
            )
            .map_err(|source| decode_error(&self.path, source))?;
            self.resampler = Some(Resampler {
                context,
                input: key,
            });
        }
        // Room for every sample this frame turns into, plus what the resampler held back.
        let capacity = decoded.samples() * self.rate as usize / decoded.rate().max(1) as usize;
        let mut converted = self.output_frame(capacity + 256);
        if let Some(resampler) = self.resampler.as_mut() {
            resampler
                .context
                .run(decoded, &mut converted)
                .map_err(|source| decode_error(&self.path, source))?;
        }
        self.keep(&converted);
        Ok(())
    }

    /// Hands out what the resampler still holds at the end of the stream.
    fn flush_resampler(&mut self) -> Result<(), MediaError> {
        let Some(mut resampler) = self.resampler.take() else {
            return Ok(());
        };
        let mut converted = self.output_frame(4096);
        resampler
            .context
            .flush(&mut converted)
            .map_err(|source| decode_error(&self.path, source))?;
        self.keep(&converted);
        Ok(())
    }

    /// An empty output frame with room for `capacity` frames.
    fn output_frame(&self, capacity: usize) -> frame::Audio {
        let mut frame = frame::Audio::new(
            Sample::F32(SampleType::Packed),
            capacity,
            ChannelLayout::default(i32::from(self.channels)),
        );
        frame.set_rate(self.rate);
        frame
    }

    /// Keeps the converted samples, minus any before the seek target.
    fn keep(&mut self, converted: &frame::Audio) {
        let channels = usize::from(self.channels);
        let frames = converted.samples();
        let position = self.position.unwrap_or(self.skip_until);
        let skip = usize::try_from(self.skip_until - position)
            .unwrap_or(0)
            .min(frames);
        let bytes = &converted.data(0)[skip * channels * 4..frames * channels * 4];
        let (samples, _) = bytes.as_chunks::<4>();
        self.pending
            .extend(samples.iter().map(|sample| f32::from_le_bytes(*sample)));
        self.position = Some(position + frames as i64);
    }

    /// A timestamp in the stream's time base, in microseconds from the container's start.
    fn micros(&self, timestamp: i64) -> i64 {
        let numerator = i128::from(timestamp) * 1_000_000 * i128::from(self.time_base.numerator());
        let denominator = i128::from(self.time_base.denominator()).max(1);
        let micros = (2 * numerator + numerator.signum() * denominator) / (2 * denominator);
        i64::try_from(micros).unwrap_or(i64::MAX) - self.start
    }

    /// The output frame index at `micros` microseconds, rounded to the nearest.
    fn frame_index(&self, micros: i64) -> i64 {
        let index = (i128::from(micros) * i128::from(self.rate) + 500_000) / 1_000_000;
        i64::try_from(index).unwrap_or(i64::MAX)
    }
}

fn decode_error(path: &Path, source: ffmpeg::Error) -> MediaError {
    MediaError::Decode {
        path: path.to_path_buf(),
        source,
    }
}
