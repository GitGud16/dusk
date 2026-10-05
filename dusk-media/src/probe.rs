use std::fmt;
use std::path::Path;

use ffmpeg_next as ffmpeg;
use ffmpeg_next::media::Type as Medium;

use dusk_core::Orientation;

use crate::MediaError;
use crate::ffi;
use crate::input::open_input;
use crate::orientation::from_display_matrix;

/// What a media file contains, read from its container without decoding any frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeInfo {
    /// FFmpeg's name for the container format, for example `mov,mp4,m4a,3gp,3g2,mj2`.
    pub format: String,
    /// Container duration in microseconds, if the file states one.
    pub duration_us: Option<i64>,
    /// The streams, in file order.
    pub streams: Vec<StreamSummary>,
}

/// One stream of a probed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSummary {
    /// The stream's index in the file.
    pub index: usize,
    /// What kind of stream it is.
    pub kind: StreamKind,
    /// FFmpeg's codec name, for example `h264` or `aac`.
    pub codec: String,
    /// Kind-specific details.
    pub detail: StreamDetail,
}

/// The kind of a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// Video frames.
    Video,
    /// Audio samples.
    Audio,
    /// Subtitles.
    Subtitle,
    /// Timed data, such as timecode or GPS tracks.
    Data,
    /// Attachments, such as fonts in Matroska.
    Attachment,
    /// Anything FFmpeg does not classify.
    Unknown,
}

/// Kind-specific details of a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamDetail {
    /// A video stream.
    Video {
        /// Coded width in pixels.
        width: u32,
        /// Coded height in pixels.
        height: u32,
        /// Average frame rate as numerator and denominator, if known.
        frame_rate: Option<(i32, i32)>,
        /// The base frame rate FFmpeg guesses from the timestamps (`r_frame_rate`), if known.
        /// It differs from the average when the frame rate varies.
        base_frame_rate: Option<(i32, i32)>,
        /// Whether the stream is an attached picture (cover art) rather than video.
        cover_art: bool,
        /// How its frames are turned and mirrored for display, from its display matrix.
        orientation: Orientation,
    },
    /// An audio stream.
    Audio {
        /// Sample rate in Hz.
        sample_rate: u32,
        /// Number of channels.
        channels: u16,
    },
    /// Any other stream.
    Other,
}

/// Reads the container and stream information of the local file at `path`.
///
/// Only local files are opened, never a URL (see [`crate::input`]).
pub fn probe(path: &Path) -> Result<ProbeInfo, MediaError> {
    let input = open_input(path)?;
    Ok(ProbeInfo {
        format: input.format().name().to_owned(),
        // FFmpeg reports durations in AV_TIME_BASE units, which are microseconds; an unknown
        // duration is AV_NOPTS_VALUE, a negative number.
        duration_us: Some(input.duration()).filter(|us| *us >= 0),
        streams: input.streams().map(summarize).collect(),
    })
}

fn summarize(stream: ffmpeg::format::stream::Stream<'_>) -> StreamSummary {
    let parameters = stream.parameters();
    let kind = match parameters.medium() {
        Medium::Video => StreamKind::Video,
        Medium::Audio => StreamKind::Audio,
        Medium::Subtitle => StreamKind::Subtitle,
        Medium::Data => StreamKind::Data,
        Medium::Attachment => StreamKind::Attachment,
        Medium::Unknown => StreamKind::Unknown,
    };
    let fields = ffi::codec_fields(&parameters);
    let detail = match kind {
        StreamKind::Video => {
            let known = |rate: ffmpeg::Rational| {
                (rate.numerator() > 0 && rate.denominator() > 0)
                    .then(|| (rate.numerator(), rate.denominator()))
            };
            StreamDetail::Video {
                width: u32::try_from(fields.width).unwrap_or(0),
                height: u32::try_from(fields.height).unwrap_or(0),
                frame_rate: known(stream.avg_frame_rate()),
                base_frame_rate: known(stream.rate()),
                cover_art: stream
                    .disposition()
                    .contains(ffmpeg::format::stream::Disposition::ATTACHED_PIC),
                orientation: ffi::display_matrix(&parameters)
                    .map_or(Orientation::UPRIGHT, |matrix| from_display_matrix(&matrix)),
            }
        }
        StreamKind::Audio => StreamDetail::Audio {
            sample_rate: u32::try_from(fields.sample_rate).unwrap_or(0),
            channels: u16::try_from(fields.channels).unwrap_or(0),
        },
        _ => StreamDetail::Other,
    };
    StreamSummary {
        index: stream.index(),
        kind,
        codec: parameters.id().name().to_owned(),
        detail,
    }
}

impl fmt::Display for ProbeInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format)?;
        match self.duration_us {
            Some(us) => write!(f, ", {}.{:03} s", us / 1_000_000, (us % 1_000_000) / 1_000)?,
            None => write!(f, ", unknown duration")?,
        }
        for stream in &self.streams {
            write!(f, "\n  {stream}")?;
        }
        Ok(())
    }
}

impl fmt::Display for StreamSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{} {} {}", self.index, self.kind, self.codec)?;
        match self.detail {
            StreamDetail::Video {
                width,
                height,
                frame_rate,
                ..
            } => {
                write!(f, " {width}x{height}")?;
                if let Some((numerator, denominator)) = frame_rate {
                    write!(f, " {numerator}/{denominator} fps")?;
                }
                Ok(())
            }
            StreamDetail::Audio {
                sample_rate,
                channels,
            } => write!(f, " {sample_rate} Hz {channels} ch"),
            StreamDetail::Other => Ok(()),
        }
    }
}

impl fmt::Display for StreamKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StreamKind::Video => "video",
            StreamKind::Audio => "audio",
            StreamKind::Subtitle => "subtitle",
            StreamKind::Data => "data",
            StreamKind::Attachment => "attachment",
            StreamKind::Unknown => "unknown",
        })
    }
}
