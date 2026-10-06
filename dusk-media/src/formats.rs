//! What an export can write (docs/ARCHITECTURE.md, "Export details"): the containers and the
//! codecs each one takes, the encoders of each codec in the order they are tried with the
//! limits FFmpeg cannot report, and how the one Quality setting maps onto each encoder's own
//! rate control.

use ffmpeg_next::format::Pixel;

use crate::decode::LARGE_FRAME;

/// A file format for video with sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Container {
    Mp4,
    Mov,
    Mkv,
    WebM,
}

/// A video codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
    Vp9,
}

/// A sound codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioCodec {
    Aac,
    Opus,
    Mp3,
    /// 16-bit PCM, as WAV files hold it.
    Pcm,
}

/// A file format for sound alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioFormat {
    Mp3,
    /// AAC in an MPEG-4 audio file.
    M4a,
    /// Opus in an Ogg file.
    Opus,
    Wav,
}

impl Container {
    /// Every container, in the order the export dialog lists them.
    pub const ALL: [Container; 4] = [
        Container::Mp4,
        Container::Mov,
        Container::Mkv,
        Container::WebM,
    ];

    /// The file name extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
            Container::Mkv => "mkv",
            Container::WebM => "webm",
        }
    }

    /// The name people know it by.
    pub fn name(self) -> &'static str {
        match self {
            Container::Mp4 => "MP4",
            Container::Mov => "MOV",
            Container::Mkv => "MKV",
            Container::WebM => "WebM",
        }
    }

    /// The video codecs it holds, the usual one first.
    pub fn video_codecs(self) -> &'static [VideoCodec] {
        use VideoCodec::*;
        match self {
            Container::Mp4 => &[H264, Hevc, Av1],
            Container::Mov => &[H264, Hevc],
            Container::Mkv => &[H264, Hevc, Av1, Vp9],
            Container::WebM => &[Vp9, Av1],
        }
    }

    /// The sound codecs it holds, the usual one first.
    pub fn audio_codecs(self) -> &'static [AudioCodec] {
        match self {
            Container::Mp4 | Container::Mov => &[AudioCodec::Aac],
            Container::Mkv => &[AudioCodec::Aac, AudioCodec::Opus],
            Container::WebM => &[AudioCodec::Opus],
        }
    }

    /// FFmpeg's muxer for it.
    /// FFmpeg's name for its muxer.
    pub fn muxer(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
            Container::Mkv => "matroska",
            Container::WebM => "webm",
        }
    }
}

impl AudioFormat {
    /// Every sound format, in the order the export dialog lists them.
    pub const ALL: [AudioFormat; 4] = [
        AudioFormat::Mp3,
        AudioFormat::M4a,
        AudioFormat::Opus,
        AudioFormat::Wav,
    ];

    /// The file name extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::M4a => "m4a",
            AudioFormat::Opus => "opus",
            AudioFormat::Wav => "wav",
        }
    }

    /// The name people know it by.
    pub fn name(self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "MP3",
            AudioFormat::M4a => "AAC",
            AudioFormat::Opus => "Opus",
            AudioFormat::Wav => "WAV",
        }
    }

    /// The codec it holds.
    pub fn codec(self) -> AudioCodec {
        match self {
            AudioFormat::Mp3 => AudioCodec::Mp3,
            AudioFormat::M4a => AudioCodec::Aac,
            AudioFormat::Opus => AudioCodec::Opus,
            AudioFormat::Wav => AudioCodec::Pcm,
        }
    }

    /// FFmpeg's muxer for it: `ipod` is MPEG-4 audio, `opus` is Ogg with Opus.
    pub(crate) fn muxer(self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::M4a => "ipod",
            AudioFormat::Opus => "opus",
            AudioFormat::Wav => "wav",
        }
    }
}

impl VideoCodec {
    /// The name people know it by.
    pub fn name(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "HEVC",
            VideoCodec::Av1 => "AV1",
            VideoCodec::Vp9 => "VP9",
        }
    }
}

/// How the video's quality is set (docs/ARCHITECTURE.md, "Quality").
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Quality {
    /// The Quality slider, from 0 to 100.
    Level(u8),
    /// A target bitrate in bits a second, which any encoder takes (Advanced).
    Bitrate(u64),
    /// A raw CRF, for an encoder that has one (Advanced); an encoder without one uses
    /// [`Quality::HIGH`].
    Crf(u8),
}

impl Quality {
    /// The High preset.
    pub const HIGH: Quality = Quality::Level(80);
    /// The Medium preset.
    pub const MEDIUM: Quality = Quality::Level(60);
    /// The Small preset.
    pub const SMALL: Quality = Quality::Level(40);
}

/// How an encoder's rate is controlled, which decides what each quality turns into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Nvenc,
    Qsv,
    Amf,
    OpenH264,
    Kvazaar,
    SvtAv1,
    Vpx,
    /// x264 and x265, in a user's own `ffmpeg`.
    X26x,
}

/// An encoder Dusk can use, with the limits FFmpeg cannot report (docs/ARCHITECTURE.md,
/// "Encoder constraints"). They are fixed, conservative values; an encoder that will not
/// open anyway is skipped for the next one.
#[derive(Debug)]
pub struct Encoder {
    /// FFmpeg's name for it, such as `h264_amf`.
    pub name: &'static str,
    pub codec: VideoCodec,
    /// Whether it runs on the graphics card.
    pub hardware: bool,
    /// The picture layout it takes.
    pub(crate) pixel: Pixel,
    /// The longest side, in pixels.
    pub(crate) max_side: u32,
    /// Pixels a frame.
    pub(crate) max_area: u64,
    /// Luma samples a second.
    pub(crate) max_luma_rate: u64,
    /// Each side a multiple of this.
    pub(crate) alignment: u32,
    control: Control,
}

/// What an encoder is opened with for a quality.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RateControl {
    /// Bits a second; 0 for none.
    pub bit_rate: usize,
    /// The peak, bits a second; 0 for none.
    pub max_rate: usize,
    /// The rate control buffer, bits; 0 for the encoder's own.
    pub buffer: i32,
    /// The quality FFmpeg's `global_quality` carries, for encoders that read it.
    pub global_quality: Option<i32>,
    /// The encoder's private options.
    pub options: Vec<(&'static str, String)>,
}

impl Encoder {
    /// Whether it takes a raw CRF (Advanced): SVT-AV1, VP9, and x264 and x265.
    pub fn has_crf(&self) -> bool {
        matches!(self.control, Control::SvtAv1 | Control::Vpx | Control::X26x)
    }

    /// Its options for `quality`, writing frames of `size` at `fps`, as a user's own `ffmpeg`
    /// takes them on its command line (docs/ARCHITECTURE.md, "Optional GPL encoders"): each
    /// name, without its dash, and its value, starting with the pixel format it takes.
    pub fn command_line(
        &self,
        quality: Quality,
        size: (u32, u32),
        fps: f64,
    ) -> Vec<(&'static str, String)> {
        let control = self.rate_control(quality, size, fps);
        let pixel = self
            .pixel
            .descriptor()
            .map_or("yuv420p", |pixel| pixel.name());
        let mut options = vec![("pix_fmt", pixel.to_owned())];
        if control.bit_rate > 0 {
            options.push(("b:v", control.bit_rate.to_string()));
        }
        if control.max_rate > 0 {
            options.push(("maxrate", control.max_rate.to_string()));
        }
        if control.buffer > 0 {
            options.push(("bufsize", control.buffer.to_string()));
        }
        if let Some(quality) = control.global_quality {
            options.push(("global_quality:v", quality.to_string()));
        }
        options.extend(control.options);
        options
    }

    /// The largest frame size it takes with the shape of `wanted` at `fps` frames a second,
    /// rounded down to its alignment.
    pub fn fit(&self, wanted: (u32, u32), fps: f64) -> (u32, u32) {
        let (width, height) = (f64::from(wanted.0.max(1)), f64::from(wanted.1.max(1)));
        let side = f64::from(self.max_side);
        let area = (self.max_area as f64).min(self.max_luma_rate as f64 / fps.max(1.0));
        let scale = (side / width)
            .min(side / height)
            .min((area / (width * height)).sqrt())
            .min(1.0);
        let align = |length: f64| {
            let length = (length * scale).floor() as u32;
            (length / self.alignment * self.alignment).max(self.alignment)
        };
        (align(width), align(height))
    }

    /// What it is opened with for `quality`, writing frames of `size` at `fps`.
    pub(crate) fn rate_control(&self, quality: Quality, size: (u32, u32), fps: f64) -> RateControl {
        let level = match quality {
            Quality::Level(level) => level.min(100),
            Quality::Crf(crf) if self.has_crf() => return self.constant_rate_factor(crf, size),
            Quality::Crf(_) => 80,
            Quality::Bitrate(bits) => return self.target(bits, size),
        };
        let level = f64::from(level);
        // H.264 and HEVC quantizers run from 0 to 51, AV1's and VP9's from 0 to 63; the
        // slider covers the useful part of each, its High preset (80) where M1 exported.
        let h26x = (37.0 - 0.2 * level).round() as i32;
        let av1 = (51.0 - 0.3 * level).round() as i32;
        let step = if matches!(self.codec, VideoCodec::Av1 | VideoCodec::Vp9) {
            av1
        } else {
            h26x
        };
        let mut control = RateControl::default();
        match self.control {
            Control::Nvenc => {
                control.options = vec![
                    ("preset", "p5".to_owned()),
                    ("rc", "vbr".to_owned()),
                    ("cq", step.to_string()),
                ];
            }
            Control::Qsv => {
                control.options = vec![("preset", "medium".to_owned())];
                control.global_quality = Some(h26x);
            }
            Control::Amf => {
                // AV1 on AMF counts its quantizer from 0 to 255.
                let qp = |offset: i32| {
                    if self.codec == VideoCodec::Av1 {
                        ((av1 + offset) * 4).clamp(0, 255)
                    } else {
                        (h26x + offset).clamp(0, 51)
                    }
                };
                control.options = vec![
                    ("usage", "transcoding".to_owned()),
                    ("quality", "quality".to_owned()),
                    ("rc", "cqp".to_owned()),
                    ("qp_i", qp(-1).to_string()),
                    ("qp_p", qp(1).to_string()),
                ];
                if self.codec == VideoCodec::H264 {
                    control.options.push(("qp_b", qp(3).to_string()));
                }
            }
            Control::OpenH264 => {
                // No constant-quality mode: bits a pixel, 0.15 at High.
                let bits_per_pixel = 0.03 + 0.0015 * level;
                control.bit_rate =
                    (bits_per_pixel * f64::from(size.0) * f64::from(size.1) * fps) as usize;
                control.options = vec![
                    ("rc_mode", "bitrate".to_owned()),
                    ("allow_skip_frames", "0".to_owned()),
                ];
            }
            Control::Kvazaar => {
                // FFmpeg splits these at commas.
                control.options = vec![("kvazaar-params", format!("preset=fast,qp={h26x}"))];
            }
            Control::SvtAv1 | Control::Vpx => return self.constant_rate_factor(av1 as u8, size),
            Control::X26x => return self.constant_rate_factor(h26x as u8, size),
        }
        control
    }

    /// A raw CRF, for SVT-AV1, VP9, x264 and x265, writing frames of `size`.
    fn constant_rate_factor(&self, crf: u8, size: (u32, u32)) -> RateControl {
        let mut control = RateControl {
            options: vec![("crf", crf.min(63).to_string())],
            ..RateControl::default()
        };
        control.options.extend(self.speed(size));
        control
    }

    /// A target bitrate, peaking at 1.2 times it with a buffer of one peak second
    /// (docs/ARCHITECTURE.md, "Target file size"). SVT-AV1 takes a peak only in its
    /// constant-quality mode and refuses to open with one otherwise, so it gets the target
    /// alone with a buffer of one second.
    fn target(&self, bits: u64, size: (u32, u32)) -> RateControl {
        let bit_rate = usize::try_from(bits).unwrap_or(usize::MAX);
        let max_rate = match self.control {
            Control::SvtAv1 => 0,
            _ => bit_rate.saturating_add(bit_rate / 5),
        };
        let mut control = RateControl {
            bit_rate,
            max_rate,
            buffer: i32::try_from(max_rate.max(bit_rate)).unwrap_or(i32::MAX),
            ..RateControl::default()
        };
        control.options = match self.control {
            Control::Nvenc => vec![("preset", "p5".to_owned()), ("rc", "vbr".to_owned())],
            Control::Qsv => vec![("preset", "medium".to_owned())],
            Control::Amf => vec![
                ("usage", "transcoding".to_owned()),
                ("quality", "quality".to_owned()),
                ("rc", "vbr_peak".to_owned()),
            ],
            Control::OpenH264 => vec![
                ("rc_mode", "bitrate".to_owned()),
                ("allow_skip_frames", "0".to_owned()),
            ],
            Control::Kvazaar => vec![("kvazaar-params", "preset=fast".to_owned())],
            Control::SvtAv1 | Control::Vpx | Control::X26x => self.speed(size),
        };
        control
    }

    /// How hard the software encoders that have presets work, writing frames of `size`: fast
    /// enough to export at a useful pace, in a bounded amount of memory (docs/ARCHITECTURE.md,
    /// "Quality"). SVT-AV1 left to itself works on as many pictures at once as there are
    /// cores for, which held 1.3 GB at 1080p and 4.7 GB at 4K at M4; at most two at once
    /// (and above the 1080p class one, without its lookahead) it held 0.5 GB and 1.0 GB.
    /// FFmpeg's libraries default to one thread, which libvpx takes as it is.
    fn speed(&self, size: (u32, u32)) -> Vec<(&'static str, String)> {
        match self.control {
            Control::SvtAv1 => {
                let large = u64::from(size.0) * u64::from(size.1) > LARGE_FRAME;
                let parallel = if large { "lp=1:lookahead=0" } else { "lp=2" };
                vec![
                    ("preset", "8".to_owned()),
                    ("svtav1-params", parallel.to_owned()),
                ]
            }
            Control::Vpx => vec![
                ("deadline", "good".to_owned()),
                ("cpu-used", "4".to_owned()),
                ("row-mt", "1".to_owned()),
                ("threads", "4".to_owned()),
            ],
            _ => Vec::new(),
        }
    }
}

/// H.264 at level 5.2: 9.4 Mpx a frame, about 531 M luma samples a second.
const H264_AREA: u64 = 9_437_184;
const H264_LUMA_RATE: u64 = 530_841_600;
/// HEVC, AV1 and VP9 at level 6.2: 35.6 Mpx a frame.
const LARGE_AREA: u64 = 35_651_584;
const LARGE_LUMA_RATE: u64 = 4_278_190_080;
/// Older AMD cards cap H.264 and HEVC at 4096x2160.
const AMF_AREA: u64 = 8_847_360;

const fn encoder(
    name: &'static str,
    codec: VideoCodec,
    control: Control,
    max_side: u32,
    max_area: u64,
    max_luma_rate: u64,
) -> Encoder {
    let hardware = matches!(control, Control::Nvenc | Control::Qsv | Control::Amf);
    Encoder {
        name,
        codec,
        hardware,
        pixel: if hardware {
            Pixel::NV12
        } else {
            Pixel::YUV420P
        },
        max_side,
        max_area,
        max_luma_rate,
        alignment: if matches!(control, Control::Kvazaar) {
            8
        } else {
            2
        },
        control,
    }
}

/// Every encoder; each codec's in the order they are tried (CLAUDE.md, "Stack").
pub static ENCODERS: [Encoder; 13] = [
    encoder(
        "h264_nvenc",
        VideoCodec::H264,
        Control::Nvenc,
        4096,
        H264_AREA,
        H264_LUMA_RATE,
    ),
    encoder(
        "h264_qsv",
        VideoCodec::H264,
        Control::Qsv,
        4096,
        H264_AREA,
        H264_LUMA_RATE,
    ),
    encoder(
        "h264_amf",
        VideoCodec::H264,
        Control::Amf,
        4096,
        AMF_AREA,
        H264_LUMA_RATE,
    ),
    encoder(
        "libopenh264",
        VideoCodec::H264,
        Control::OpenH264,
        4096,
        H264_AREA,
        H264_LUMA_RATE,
    ),
    encoder(
        "hevc_nvenc",
        VideoCodec::Hevc,
        Control::Nvenc,
        8192,
        LARGE_AREA,
        LARGE_LUMA_RATE,
    ),
    encoder(
        "hevc_qsv",
        VideoCodec::Hevc,
        Control::Qsv,
        8192,
        LARGE_AREA,
        LARGE_LUMA_RATE,
    ),
    encoder(
        "hevc_amf",
        VideoCodec::Hevc,
        Control::Amf,
        4096,
        AMF_AREA,
        H264_LUMA_RATE,
    ),
    encoder(
        "libkvazaar",
        VideoCodec::Hevc,
        Control::Kvazaar,
        8192,
        LARGE_AREA,
        LARGE_LUMA_RATE,
    ),
    encoder(
        "av1_nvenc",
        VideoCodec::Av1,
        Control::Nvenc,
        8192,
        LARGE_AREA,
        LARGE_LUMA_RATE,
    ),
    encoder(
        "av1_qsv",
        VideoCodec::Av1,
        Control::Qsv,
        8192,
        LARGE_AREA,
        LARGE_LUMA_RATE,
    ),
    encoder(
        "av1_amf",
        VideoCodec::Av1,
        Control::Amf,
        8192,
        LARGE_AREA,
        LARGE_LUMA_RATE,
    ),
    encoder(
        "libsvtav1",
        VideoCodec::Av1,
        Control::SvtAv1,
        8192,
        LARGE_AREA,
        LARGE_LUMA_RATE,
    ),
    encoder(
        "libvpx-vp9",
        VideoCodec::Vp9,
        Control::Vpx,
        16384,
        LARGE_AREA,
        LARGE_LUMA_RATE,
    ),
];

/// The GPL encoders Dusk reaches only through a user's own `ffmpeg` (docs/ARCHITECTURE.md,
/// "Optional GPL encoders"). They are not in the FFmpeg Dusk ships, so they are never tried
/// for an export of its own; their limits are their codecs' levels.
pub static EXTERNAL_ENCODERS: [Encoder; 2] = [
    encoder(
        "libx264",
        VideoCodec::H264,
        Control::X26x,
        8192,
        H264_AREA,
        H264_LUMA_RATE,
    ),
    encoder(
        "libx265",
        VideoCodec::Hevc,
        Control::X26x,
        8192,
        LARGE_AREA,
        LARGE_LUMA_RATE,
    ),
];

/// The encoders of `codec`, in the order they are tried.
pub fn encoders_of(codec: VideoCodec) -> impl Iterator<Item = &'static Encoder> {
    ENCODERS
        .iter()
        .filter(move |encoder| encoder.codec == codec)
}

/// The encoder called `name`.
pub fn encoder_named(name: &str) -> Option<&'static Encoder> {
    ENCODERS.iter().find(|encoder| encoder.name == name)
}

/// The GPL encoder called `name`, from [`EXTERNAL_ENCODERS`].
pub fn external_encoder_named(name: &str) -> Option<&'static Encoder> {
    EXTERNAL_ENCODERS
        .iter()
        .find(|encoder| encoder.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(codec: VideoCodec) -> Vec<&'static str> {
        encoders_of(codec).map(|encoder| encoder.name).collect()
    }

    fn option<'a>(control: &'a RateControl, name: &str) -> Option<&'a str> {
        control
            .options
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }

    fn named(name: &str) -> &'static Encoder {
        encoder_named(name).unwrap_or_else(|| panic!("{name} is in the table"))
    }

    const HD: (u32, u32) = (1920, 1080);

    #[test]
    fn each_container_takes_the_documented_codecs() {
        use VideoCodec::*;
        assert_eq!(Container::Mp4.video_codecs(), [H264, Hevc, Av1]);
        assert_eq!(Container::Mov.video_codecs(), [H264, Hevc]);
        assert_eq!(Container::Mkv.video_codecs(), [H264, Hevc, Av1, Vp9]);
        // WebM never holds H.264.
        assert_eq!(Container::WebM.video_codecs(), [Vp9, Av1]);
        assert_eq!(Container::Mp4.audio_codecs(), [AudioCodec::Aac]);
        assert_eq!(Container::Mov.audio_codecs(), [AudioCodec::Aac]);
        assert_eq!(Container::WebM.audio_codecs(), [AudioCodec::Opus]);
        assert_eq!(
            Container::Mkv.audio_codecs(),
            [AudioCodec::Aac, AudioCodec::Opus]
        );
        let extensions: Vec<&str> = Container::ALL.iter().map(|c| c.extension()).collect();
        assert_eq!(extensions, ["mp4", "mov", "mkv", "webm"]);
        let muxers: Vec<&str> = Container::ALL.iter().map(|c| c.muxer()).collect();
        assert_eq!(muxers, ["mp4", "mov", "matroska", "webm"]);
    }

    #[test]
    fn sound_files_hold_one_codec_each() {
        let formats: Vec<(&str, AudioCodec, &str)> = AudioFormat::ALL
            .iter()
            .map(|f| (f.extension(), f.codec(), f.muxer()))
            .collect();
        assert_eq!(
            formats,
            [
                ("mp3", AudioCodec::Mp3, "mp3"),
                ("m4a", AudioCodec::Aac, "ipod"),
                ("opus", AudioCodec::Opus, "opus"),
                ("wav", AudioCodec::Pcm, "wav"),
            ]
        );
        assert_eq!(AudioFormat::M4a.name(), "AAC");
    }

    #[test]
    fn encoders_are_tried_in_the_documented_order() {
        assert_eq!(
            names(VideoCodec::H264),
            ["h264_nvenc", "h264_qsv", "h264_amf", "libopenh264"]
        );
        assert_eq!(
            names(VideoCodec::Hevc),
            ["hevc_nvenc", "hevc_qsv", "hevc_amf", "libkvazaar"]
        );
        assert_eq!(
            names(VideoCodec::Av1),
            ["av1_nvenc", "av1_qsv", "av1_amf", "libsvtav1"]
        );
        assert_eq!(names(VideoCodec::Vp9), ["libvpx-vp9"]);
        let software: Vec<&str> = ENCODERS
            .iter()
            .filter(|encoder| !encoder.hardware)
            .map(|encoder| encoder.name)
            .collect();
        assert_eq!(
            software,
            ["libopenh264", "libkvazaar", "libsvtav1", "libvpx-vp9"]
        );
        assert_eq!(VideoCodec::Hevc.name(), "HEVC");
    }

    #[test]
    fn only_svt_av1_and_vp9_take_a_crf() {
        let with_crf: Vec<&str> = ENCODERS
            .iter()
            .filter(|encoder| encoder.has_crf())
            .map(|encoder| encoder.name)
            .collect();
        assert_eq!(with_crf, ["libsvtav1", "libvpx-vp9"]);
    }

    #[test]
    fn frames_keep_their_shape_inside_each_encoders_limits() {
        // H.264 encoders cap each side at 4096, so a wide frame comes down though its area
        // would pass.
        assert_eq!(named("h264_amf").fit((5120, 1440), 30.0), (4096, 1152));
        // Kvazaar wants multiples of 8.
        assert_eq!(named("libkvazaar").fit((1918, 1078), 30.0), (1912, 1072));
        // H.264 level 5.2 holds 4096x2160 at 60 fps, but not 4096x2304.
        assert_eq!(named("libopenh264").fit((4096, 2160), 60.0), (4096, 2160));
        let (width, height) = named("libopenh264").fit((4096, 2304), 60.0);
        assert!(u64::from(width) * u64::from(height) * 60 <= 530_841_600);
        // HEVC and AV1 take 8K.
        assert_eq!(named("libsvtav1").fit((7680, 4320), 30.0), (7680, 4320));
        // An odd size rounds down to even.
        assert_eq!(named("h264_nvenc").fit((641, 361), 30.0), (640, 360));
    }

    #[test]
    fn the_high_preset_keeps_the_settings_m1_exported_with() {
        let high = |name: &str| named(name).rate_control(Quality::HIGH, HD, 30.0);
        let nvenc = high("h264_nvenc");
        assert_eq!(option(&nvenc, "cq"), Some("21"));
        assert_eq!(option(&nvenc, "rc"), Some("vbr"));
        assert_eq!(high("h264_qsv").global_quality, Some(21));
        let amf = high("h264_amf");
        assert_eq!(
            (
                option(&amf, "qp_i"),
                option(&amf, "qp_p"),
                option(&amf, "qp_b")
            ),
            (Some("20"), Some("22"), Some("24"))
        );
        // OpenH264 has no constant quality: 0.15 bits a pixel.
        let openh264 = high("libopenh264");
        assert_eq!(openh264.bit_rate, 9_331_200);
        assert_eq!(option(&openh264, "rc_mode"), Some("bitrate"));
    }

    #[test]
    fn lower_quality_means_coarser_steps_or_fewer_bits() {
        let number = |control: &RateControl, key: &str| -> f64 {
            option(control, key)
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0)
        };
        for encoder in ENCODERS.iter().chain(&EXTERNAL_ENCODERS) {
            let at = |level: u8| encoder.rate_control(Quality::Level(level), HD, 30.0);
            let (fine, coarse) = (at(80), at(40));
            let finer = match encoder.control {
                Control::Nvenc => number(&fine, "cq") < number(&coarse, "cq"),
                Control::Qsv => fine.global_quality < coarse.global_quality,
                Control::Amf => number(&fine, "qp_p") < number(&coarse, "qp_p"),
                Control::OpenH264 => fine.bit_rate > coarse.bit_rate,
                Control::Kvazaar => {
                    option(&fine, "kvazaar-params") != option(&coarse, "kvazaar-params")
                }
                Control::SvtAv1 | Control::Vpx | Control::X26x => {
                    number(&fine, "crf") < number(&coarse, "crf")
                }
            };
            assert!(finer, "{}: {fine:?} against {coarse:?}", encoder.name);
        }
    }

    #[test]
    fn the_quality_scales_reach_their_documented_ends() {
        let svt = |level| named("libsvtav1").rate_control(Quality::Level(level), HD, 30.0);
        assert_eq!(option(&svt(80), "crf"), Some("27"));
        assert_eq!(option(&svt(0), "crf"), Some("51"));
        assert_eq!(option(&svt(100), "crf"), Some("21"));
        let kvazaar = named("libkvazaar").rate_control(Quality::HIGH, HD, 30.0);
        assert_eq!(
            option(&kvazaar, "kvazaar-params"),
            Some("preset=fast,qp=21")
        );
        let vp9 = named("libvpx-vp9").rate_control(Quality::MEDIUM, HD, 30.0);
        assert_eq!(option(&vp9, "crf"), Some("33"));
        // Constant quality in libvpx needs a bitrate of 0.
        assert_eq!(vp9.bit_rate, 0);
    }

    #[test]
    fn a_target_bitrate_peaks_at_1_2_times_with_a_matching_buffer() {
        for encoder in ENCODERS
            .iter()
            .filter(|encoder| encoder.name != "libsvtav1")
        {
            let control = encoder.rate_control(Quality::Bitrate(4_000_000), HD, 30.0);
            assert_eq!(
                (control.bit_rate, control.max_rate, control.buffer),
                (4_000_000, 4_800_000, 4_800_000),
                "{}",
                encoder.name
            );
        }
        // SVT-AV1 refuses a peak outside its constant-quality mode.
        let svt = named("libsvtav1").rate_control(Quality::Bitrate(4_000_000), HD, 30.0);
        assert_eq!(
            (svt.bit_rate, svt.max_rate, svt.buffer),
            (4_000_000, 0, 4_000_000)
        );
        let amf = named("hevc_amf").rate_control(Quality::Bitrate(4_000_000), HD, 30.0);
        assert_eq!(option(&amf, "rc"), Some("vbr_peak"));
    }

    #[test]
    fn a_crf_reaches_only_the_encoders_that_have_one() {
        let svt = named("libsvtav1").rate_control(Quality::Crf(35), HD, 30.0);
        assert_eq!(option(&svt, "crf"), Some("35"));
        let nvenc = named("h264_nvenc");
        assert_eq!(
            nvenc.rate_control(Quality::Crf(35), HD, 30.0),
            nvenc.rate_control(Quality::HIGH, HD, 30.0)
        );
    }

    /// Every way the quality can be set.
    const QUALITIES: [Quality; 3] = [Quality::HIGH, Quality::Crf(30), Quality::Bitrate(4_000_000)];

    #[test]
    fn svt_av1_works_on_fewer_pictures_at_once_to_bound_its_memory() {
        // Left to itself it held 1.3 GB at 1080p and 4.7 GB at 4K (M4).
        let svt = named("libsvtav1");
        for quality in QUALITIES {
            let hd = svt.rate_control(quality, HD, 30.0);
            assert_eq!(option(&hd, "svtav1-params"), Some("lp=2"), "{quality:?}");
            // Above the 1080p class, one at a time and without its lookahead.
            let uhd = svt.rate_control(quality, (3840, 2160), 30.0);
            assert_eq!(
                option(&uhd, "svtav1-params"),
                Some("lp=1:lookahead=0"),
                "{quality:?}"
            );
        }
        let edge = svt.rate_control(Quality::HIGH, (1920, 1088), 30.0);
        assert_eq!(option(&edge, "svtav1-params"), Some("lp=2"));
    }

    #[test]
    fn vp9_encodes_on_four_threads() {
        // FFmpeg's libraries default to one thread, which libvpx takes as it is.
        let vp9 = named("libvpx-vp9");
        for quality in QUALITIES {
            let control = vp9.rate_control(quality, HD, 30.0);
            assert_eq!(option(&control, "threads"), Some("4"), "{quality:?}");
        }
    }

    fn external(name: &str) -> &'static Encoder {
        external_encoder_named(name).unwrap_or_else(|| panic!("{name} is in the GPL table"))
    }

    #[test]
    fn the_gpl_encoders_are_never_tried_in_the_shipped_ffmpeg() {
        let listed: Vec<(&str, VideoCodec)> = EXTERNAL_ENCODERS
            .iter()
            .map(|encoder| (encoder.name, encoder.codec))
            .collect();
        assert_eq!(
            listed,
            [("libx264", VideoCodec::H264), ("libx265", VideoCodec::Hevc)]
        );
        for encoder in &EXTERNAL_ENCODERS {
            assert!(encoder_named(encoder.name).is_none(), "{}", encoder.name);
            assert!(!encoder.hardware && encoder.has_crf(), "{}", encoder.name);
        }
        assert!(external_encoder_named("libopenh264").is_none());
        // The codecs' levels bound them, not a hardware encoder's 4096-pixel side.
        assert_eq!(external("libx264").fit((5120, 1440), 30.0), (5120, 1440));
        let (width, height) = external("libx264").fit((4096, 2304), 60.0);
        assert!(u64::from(width) * u64::from(height) * 60 <= 530_841_600);
        assert_eq!(external("libx265").fit((7680, 4320), 30.0), (7680, 4320));
    }

    /// An encoder's options for `quality` as an `ffmpeg` command line has them.
    fn command_line(encoder: &Encoder, quality: Quality) -> String {
        encoder
            .command_line(quality, HD, 30.0)
            .iter()
            .map(|(name, value)| format!("-{name} {value}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn an_external_ffmpeg_is_given_the_same_mapping_as_options() {
        // The slider maps as for every H.264 and HEVC encoder: 21 at High.
        assert_eq!(
            command_line(external("libx264"), Quality::HIGH),
            "-pix_fmt yuv420p -crf 21"
        );
        assert_eq!(
            command_line(external("libx265"), Quality::SMALL),
            "-pix_fmt yuv420p -crf 29"
        );
        assert_eq!(
            command_line(external("libx265"), Quality::Crf(30)),
            "-pix_fmt yuv420p -crf 30"
        );
        assert_eq!(
            command_line(external("libx264"), Quality::Bitrate(4_000_000)),
            "-pix_fmt yuv420p -b:v 4000000 -maxrate 4800000 -bufsize 4800000"
        );
        // The shipped encoders translate the same way.
        assert_eq!(
            command_line(named("libopenh264"), Quality::HIGH),
            "-pix_fmt yuv420p -b:v 9331200 -rc_mode bitrate -allow_skip_frames 0"
        );
        assert_eq!(
            command_line(named("h264_qsv"), Quality::HIGH),
            "-pix_fmt nv12 -global_quality:v 21 -preset medium"
        );
    }
}
