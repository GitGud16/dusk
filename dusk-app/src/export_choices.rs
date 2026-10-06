//! The export dialog's choices (docs/ARCHITECTURE.md, "Export details"): only what the
//! loaded FFmpeg can produce on this machine is offered, the encoder each codec will use is
//! shown, size presets never enlarge the picture, and the choices become the engine's
//! export settings. A user's own `ffmpeg` can write the codecs it has a GPL encoder for
//! ("Optional GPL encoders").

use dusk_engine::{
    AudioCodec, AudioFormat, Container, Encoder, ExportFormat, ExportSettings, ExternalEncoder,
    Quality, VideoCodec, export_size,
};

/// The short sides the size presets offer, largest first.
pub const PRESETS: [u32; 3] = [1080, 720, 480];

/// What the export dialog offers and what is chosen in it.
#[derive(Clone, Debug)]
pub struct ExportChoices {
    /// The encoders that open on this machine, in the order they are tried.
    available: Vec<&'static Encoder>,
    /// The sequence's picture size.
    sequence: (u32, u32),
    /// Whether there is a picture to export, and sound.
    has_video: bool,
    has_sound: bool,
    /// Sound alone, rather than a video file.
    pub sound_only: bool,
    pub container: Container,
    pub codec: VideoCodec,
    /// The sound codec of a video file.
    pub audio: AudioCodec,
    /// The format of sound alone.
    pub sound_format: AudioFormat,
    /// The picture's short side; `None` for the sequence's size.
    pub short_side: Option<u32>,
    /// The Quality slider, 0 to 100.
    pub level: u8,
    /// Advanced: a target bitrate in kbit/s, which takes over from the slider.
    pub bitrate_kbps: Option<u32>,
    /// Advanced: a raw CRF, for an encoder that has one; it takes over from the rest.
    pub crf: Option<u8>,
    /// The GPL encoders of the user's own `ffmpeg`, when one was picked.
    pub external: Vec<ExternalEncoder>,
    /// Whether that program writes the codecs it has an encoder for.
    pub use_external: bool,
}

impl ExportChoices {
    /// The choices for a sequence of `sequence` size, given the encoders `available`, starting
    /// from the settings of the export before when there was one.
    pub fn new(
        available: Vec<&'static Encoder>,
        sequence: (u32, u32),
        has_video: bool,
        has_sound: bool,
        before: Option<ExportSettings>,
    ) -> ExportChoices {
        let mut choices = ExportChoices {
            available,
            sequence,
            has_video,
            has_sound,
            sound_only: false,
            container: Container::Mp4,
            codec: VideoCodec::H264,
            audio: AudioCodec::Aac,
            sound_format: AudioFormat::Mp3,
            short_side: None,
            level: 80,
            bitrate_kbps: None,
            crf: None,
            external: Vec::new(),
            use_external: false,
        };
        if let Some(before) = before {
            match before.format {
                ExportFormat::Video {
                    container,
                    codec,
                    audio,
                } => (choices.container, choices.codec, choices.audio) = (container, codec, audio),
                ExportFormat::Sound(format) => {
                    choices.sound_only = true;
                    choices.sound_format = format;
                }
            }
            match before.quality {
                Quality::Level(level) => choices.level = level.min(100),
                Quality::Bitrate(bits) => {
                    choices.bitrate_kbps = u32::try_from(bits / 1000).ok();
                }
                Quality::Crf(crf) => choices.crf = Some(crf),
            }
            choices.short_side = before
                .short_side
                .filter(|side| choices.sizes().contains(&Some(*side)));
        }
        // Only what can be made here.
        let containers = choices.containers();
        if !containers.contains(&choices.container) {
            choices.container = containers.first().copied().unwrap_or(Container::Mp4);
        }
        choices.set_container(choices.container);
        if !choices.video_offered() {
            choices.sound_only = true;
        } else if !choices.sound_offered() {
            choices.sound_only = false;
        }
        choices
    }

    fn has_encoder(&self, codec: VideoCodec) -> bool {
        self.available.iter().any(|encoder| encoder.codec == codec)
    }

    /// The containers that can hold a codec with an encoder here.
    pub fn containers(&self) -> Vec<Container> {
        Container::ALL
            .into_iter()
            .filter(|container| {
                container
                    .video_codecs()
                    .iter()
                    .any(|codec| self.has_encoder(*codec))
            })
            .collect()
    }

    /// The chosen container's codecs that have an encoder here.
    pub fn codecs(&self) -> Vec<VideoCodec> {
        self.container
            .video_codecs()
            .iter()
            .copied()
            .filter(|codec| self.has_encoder(*codec))
            .collect()
    }

    /// The encoder that will write the chosen codec: the user's own `ffmpeg`'s when it is
    /// used, else the first here, in the order tried.
    pub fn encoder(&self) -> Option<&'static Encoder> {
        if let Some(external) = self.chosen_external() {
            return Some(external.encoder);
        }
        self.available
            .iter()
            .copied()
            .find(|encoder| encoder.codec == self.codec)
    }

    /// The user's own `ffmpeg`'s encoder for the chosen codec, when it has one.
    fn external_for_codec(&self) -> Option<&ExternalEncoder> {
        self.external
            .iter()
            .find(|external| external.encoder.codec == self.codec)
    }

    /// Whether the user's own `ffmpeg` can write the chosen codec, so the choice between it
    /// and Dusk's encoder is offered.
    pub fn external_offered(&self) -> bool {
        self.external_for_codec().is_some()
    }

    /// The user's own `ffmpeg`'s encoder when it writes the video: it is used, it has an
    /// encoder for the chosen codec, and the export is a video file.
    pub fn chosen_external(&self) -> Option<&ExternalEncoder> {
        if !self.use_external || self.sound_only {
            return None;
        }
        self.external_for_codec()
    }

    /// Whether a sound-only export is offered: only when there is sound.
    pub fn sound_offered(&self) -> bool {
        self.has_sound
    }

    /// Whether a video file is offered: only when there is a picture and an encoder for it.
    pub fn video_offered(&self) -> bool {
        self.has_video && !self.containers().is_empty()
    }

    /// Whether what is chosen can be made: the sound when it is sound alone, otherwise a
    /// video file.
    pub fn ready(&self) -> bool {
        if self.sound_only {
            self.sound_offered()
        } else {
            self.video_offered()
        }
    }

    /// Whether the chosen codec's encoder takes a raw CRF.
    pub fn crf_offered(&self) -> bool {
        self.encoder().is_some_and(Encoder::has_crf)
    }

    /// The sizes offered: the sequence's own, then each preset below its short side.
    pub fn sizes(&self) -> Vec<Option<u32>> {
        let short = self.sequence.0.min(self.sequence.1);
        std::iter::once(None)
            .chain(
                PRESETS
                    .into_iter()
                    .filter(|preset| *preset < short)
                    .map(Some),
            )
            .collect()
    }

    /// The picture size of `short_side`.
    pub fn size_of(&self, short_side: Option<u32>) -> (u32, u32) {
        export_size(self.sequence, short_side)
    }

    /// Chooses `container`, keeping the codec and the sound codec when it holds them.
    pub fn set_container(&mut self, container: Container) {
        self.container = container;
        let codecs = self.codecs();
        if !codecs.contains(&self.codec)
            && let Some(first) = codecs.first()
        {
            self.codec = *first;
        }
        let sounds = container.audio_codecs();
        if !sounds.contains(&self.audio) {
            self.audio = sounds[0];
        }
    }

    /// The export settings these choices make.
    pub fn settings(&self) -> ExportSettings {
        let quality = match (self.crf.filter(|_| self.crf_offered()), self.bitrate_kbps) {
            (Some(crf), _) => Quality::Crf(crf),
            (None, Some(kbps)) if kbps > 0 => Quality::Bitrate(u64::from(kbps) * 1000),
            _ => Quality::Level(self.level),
        };
        let format = if self.sound_only {
            ExportFormat::Sound(self.sound_format)
        } else {
            ExportFormat::Video {
                container: self.container,
                codec: self.codec,
                audio: self.audio,
            }
        };
        ExportSettings {
            format,
            short_side: self.short_side,
            quality,
            audio_bit_rate: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_engine::{ENCODERS, ExternalEncoder, Quality};

    fn named(names: &[&str]) -> Vec<&'static Encoder> {
        ENCODERS
            .iter()
            .filter(|encoder| names.contains(&encoder.name))
            .collect()
    }

    /// What this machine has: AMD's H.264 and HEVC, and the software encoders.
    fn here() -> Vec<&'static Encoder> {
        named(&[
            "h264_amf",
            "libopenh264",
            "hevc_amf",
            "libkvazaar",
            "libsvtav1",
            "libvpx-vp9",
        ])
    }

    fn choices(available: Vec<&'static Encoder>) -> ExportChoices {
        ExportChoices::new(available, (1920, 1080), true, true, None)
    }

    #[test]
    fn an_export_is_ready_only_when_what_is_chosen_can_be_made() {
        assert!(choices(here()).ready());
        let nothing = ExportChoices::new(Vec::new(), (1920, 1080), true, false, None);
        assert!(!nothing.ready());
        let sound = ExportChoices::new(Vec::new(), (1920, 1080), true, true, None);
        assert!(sound.sound_only);
        assert!(sound.ready());
    }

    #[test]
    fn only_codecs_with_an_encoder_here_are_offered() {
        let mut all = choices(here());
        assert_eq!(all.containers(), Container::ALL);
        assert_eq!(
            all.codecs(),
            [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1]
        );
        all.set_container(Container::WebM);
        assert_eq!(all.codecs(), [VideoCodec::Vp9, VideoCodec::Av1]);
        // With OpenH264 alone there is nothing WebM can hold.
        let h264 = choices(named(&["libopenh264"]));
        assert_eq!(
            h264.containers(),
            [Container::Mp4, Container::Mov, Container::Mkv]
        );
        assert_eq!(h264.codecs(), [VideoCodec::H264]);
    }

    #[test]
    fn the_encoder_shown_is_the_first_here_in_the_order() {
        let mut here = choices(here());
        assert_eq!(here.encoder().map(|e| e.name), Some("h264_amf"));
        here.codec = VideoCodec::Av1;
        assert_eq!(here.encoder().map(|e| e.name), Some("libsvtav1"));
        assert!(here.crf_offered());
        here.codec = VideoCodec::H264;
        assert!(!here.crf_offered());
    }

    #[test]
    fn a_new_container_keeps_the_codec_when_it_can() {
        let mut choices = choices(here());
        choices.codec = VideoCodec::Hevc;
        choices.set_container(Container::Mov);
        assert_eq!(choices.codec, VideoCodec::Hevc);
        choices.set_container(Container::WebM);
        assert_eq!(choices.codec, VideoCodec::Vp9);
        choices.set_container(Container::Mkv);
        assert_eq!(choices.codec, VideoCodec::Vp9);
        choices.set_container(Container::Mp4);
        assert_eq!(choices.codec, VideoCodec::H264);
    }

    #[test]
    fn the_sound_codec_follows_the_container() {
        let mut choices = choices(here());
        assert_eq!(choices.audio, AudioCodec::Aac);
        choices.set_container(Container::Mkv);
        choices.audio = AudioCodec::Opus;
        choices.set_container(Container::WebM);
        assert_eq!(choices.audio, AudioCodec::Opus);
        choices.set_container(Container::Mp4);
        assert_eq!(choices.audio, AudioCodec::Aac);
    }

    #[test]
    fn size_presets_below_the_sequence_are_offered() {
        let hd = choices(here());
        assert_eq!(hd.sizes(), [None, Some(720), Some(480)]);
        assert_eq!(hd.size_of(Some(720)), (1280, 720));
        let uhd = ExportChoices::new(here(), (3840, 2160), true, true, None);
        assert_eq!(uhd.sizes(), [None, Some(1080), Some(720), Some(480)]);
        let portrait = ExportChoices::new(here(), (1080, 1920), true, true, None);
        assert_eq!(portrait.sizes(), [None, Some(720), Some(480)]);
        assert_eq!(portrait.size_of(Some(720)), (720, 1280));
    }

    #[test]
    fn the_choices_become_export_settings() {
        let mut choices = choices(here());
        choices.level = 60;
        choices.short_side = Some(720);
        let settings = choices.settings();
        assert_eq!(settings.quality, Quality::Level(60));
        assert_eq!(settings.short_side, Some(720));
        assert_eq!(
            settings.format,
            ExportFormat::Video {
                container: Container::Mp4,
                codec: VideoCodec::H264,
                audio: AudioCodec::Aac
            }
        );
        // A target bitrate takes over from the slider.
        choices.bitrate_kbps = Some(4000);
        assert_eq!(choices.settings().quality, Quality::Bitrate(4_000_000));
        // A CRF takes over from both, but only where the encoder has one.
        choices.crf = Some(30);
        assert_eq!(choices.settings().quality, Quality::Bitrate(4_000_000));
        choices.codec = VideoCodec::Av1;
        assert_eq!(choices.settings().quality, Quality::Crf(30));
        choices.sound_only = true;
        choices.sound_format = AudioFormat::Opus;
        assert_eq!(
            choices.settings().format,
            ExportFormat::Sound(AudioFormat::Opus)
        );
    }

    #[test]
    fn the_last_export_settings_are_where_the_dialog_starts() {
        let before = ExportSettings {
            format: ExportFormat::Video {
                container: Container::Mkv,
                codec: VideoCodec::Hevc,
                audio: AudioCodec::Opus,
            },
            short_side: Some(480),
            quality: Quality::Level(40),
            audio_bit_rate: None,
        };
        let choices = ExportChoices::new(here(), (1920, 1080), true, true, Some(before));
        assert_eq!(
            (
                choices.container,
                choices.codec,
                choices.audio,
                choices.short_side,
                choices.level
            ),
            (
                Container::Mkv,
                VideoCodec::Hevc,
                AudioCodec::Opus,
                Some(480),
                40
            )
        );
        // A codec with no encoder here falls back to one that has one.
        let hevc_gone = ExportChoices::new(
            named(&["libopenh264"]),
            (1920, 1080),
            true,
            true,
            Some(before),
        );
        assert_eq!(hevc_gone.codec, VideoCodec::H264);
    }

    fn program_with(names: &[&str]) -> Vec<ExternalEncoder> {
        names
            .iter()
            .map(|name| ExternalEncoder {
                program: "ffmpeg.exe".into(),
                encoder: dusk_engine::external_encoder_named(name).unwrap(),
            })
            .collect()
    }

    #[test]
    fn a_users_ffmpeg_writes_the_codecs_it_has_an_encoder_for() {
        let mut choices = choices(here());
        assert!(!choices.external_offered());
        choices.external = program_with(&["libx264"]);
        choices.use_external = true;
        assert!(choices.external_offered());
        let chosen = choices
            .chosen_external()
            .map(|external| external.encoder.name);
        assert_eq!(chosen, Some("libx264"));
        assert_eq!(choices.encoder().map(|e| e.name), Some("libx264"));
        // It has no HEVC encoder, so Dusk's own writes HEVC.
        choices.codec = VideoCodec::Hevc;
        assert!(!choices.external_offered());
        assert!(choices.chosen_external().is_none());
        assert_eq!(choices.encoder().map(|e| e.name), Some("hevc_amf"));
        // Sound alone never goes through it.
        choices.codec = VideoCodec::H264;
        choices.sound_only = true;
        assert!(choices.chosen_external().is_none());
        // Turned off, Dusk's own encoder writes the video.
        choices.sound_only = false;
        choices.use_external = false;
        assert!(choices.external_offered());
        assert!(choices.chosen_external().is_none());
        assert_eq!(choices.encoder().map(|e| e.name), Some("h264_amf"));
    }

    #[test]
    fn x264_in_a_users_ffmpeg_takes_a_crf() {
        let mut choices = choices(here());
        choices.external = program_with(&["libx264", "libx265"]);
        choices.use_external = true;
        assert!(choices.crf_offered());
        choices.crf = Some(18);
        assert_eq!(choices.settings().quality, Quality::Crf(18));
        // Dusk's own H.264 encoders have none.
        choices.use_external = false;
        assert!(!choices.crf_offered());
        assert_eq!(choices.settings().quality, Quality::Level(80));
    }

    #[test]
    fn what_there_is_decides_what_can_be_exported() {
        let silent = ExportChoices::new(here(), (1920, 1080), true, false, None);
        assert!(!silent.sound_offered() && silent.video_offered());
        let sound = ExportChoices::new(here(), (1920, 1080), false, true, None);
        assert!(sound.sound_offered() && !sound.video_offered());
        assert!(sound.sound_only);
        let nothing_encodes = ExportChoices::new(Vec::new(), (1920, 1080), true, true, None);
        assert!(!nothing_encodes.video_offered());
        assert!(nothing_encodes.sound_only);
    }
}
