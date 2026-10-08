//! The Settings dialog (docs/ARCHITECTURE.md, "Keyboard and settings"): the frame cache's
//! size, the export dialog's default and the user's own `ffmpeg.exe`. A change applies at once
//! and is kept in `settings.txt`; how each control changes the settings is worked out here.

use std::rc::Rc;

use dusk_engine::Container;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::SettingsView;
use crate::app::{App, with_app};
use crate::export_choices::PRESETS;
use crate::settings::{self, CACHE_MB, Settings};

/// The picture sizes the dialog offers, by index: the sequence's own, then each preset.
pub fn size_names() -> Vec<String> {
    std::iter::once("The sequence's own".to_owned())
        .chain(PRESETS.iter().map(|side| format!("{side}p")))
        .collect()
}

/// The index of `settings`' picture size among [`size_names`].
pub fn size_index(settings: &Settings) -> usize {
    settings
        .export
        .short_side
        .and_then(|side| PRESETS.iter().position(|preset| *preset == side))
        .map_or(0, |index| index + 1)
}

/// The dialog's `what` changed to `value` (see ui/settings-dialog.slint): "cache" in
/// megabytes, "container", "codec" and "size" by index, "level" from 0 to 100, and
/// "use-ffmpeg" 0 or 1. True when the settings changed.
pub fn change(settings: &mut Settings, what: &str, value: i32) -> bool {
    let before = settings.clone();
    let index = usize::try_from(value).ok();
    let export = &mut settings.export;
    match what {
        "cache" => {
            let megabytes = u32::try_from(value).unwrap_or(0);
            settings.cache_mb = megabytes.clamp(*CACHE_MB.start(), *CACHE_MB.end());
        }
        "container" => {
            if let Some(container) = index.and_then(|index| Container::ALL.get(index)) {
                export.container = *container;
                let codecs = container.video_codecs();
                if !codecs.contains(&export.codec)
                    && let Some(first) = codecs.first()
                {
                    export.codec = *first;
                }
            }
        }
        "codec" => {
            let codecs = export.container.video_codecs();
            if let Some(codec) = index.and_then(|index| codecs.get(index)) {
                export.codec = *codec;
            }
        }
        "size" => match index {
            Some(0) => export.short_side = None,
            Some(index) if index <= PRESETS.len() => export.short_side = Some(PRESETS[index - 1]),
            _ => {}
        },
        "level" => export.level = u8::try_from(value.clamp(0, 100)).unwrap_or(80),
        "use-ffmpeg" => settings.use_ffmpeg = value == 1,
        _ => {}
    }
    *settings != before
}

impl App {
    /// File → Settings… and Ctrl+,: the Settings dialog opens, unless another dialog is.
    pub(crate) fn open_settings(&mut self) {
        if self.main_dialog_open() {
            return;
        }
        self.settings_open = true;
        self.refresh_settings_dialog();
    }

    /// The dialog's `what` changed to `value` (see ui/settings-dialog.slint): it applies at
    /// once and is written to the settings file.
    pub fn settings_changed(&mut self, what: &str, value: i32) {
        match what {
            "program" => {
                if let Some(window) = self.window() {
                    self.pick_program_over(window.window());
                }
                return;
            }
            "forget" => return self.forget_program(),
            _ => {}
        }
        let before = self.settings.clone();
        if change(&mut self.settings, what, value) {
            if self.settings.cache_mb != before.cache_mb {
                self.engine.set_cache_cap(self.settings.cache_bytes());
            }
            // The next export starts from the new default, not from the last export.
            if self.settings.export != before.export {
                self.last_export = None;
            }
            if self.settings.use_ffmpeg != before.use_ffmpeg {
                self.use_external = self.settings.use_ffmpeg && !self.external.is_empty();
            }
            self.save_settings();
        }
        self.refresh_settings_dialog();
    }

    /// Forget: exports use Dusk's own encoders again, and the settings keep no program.
    fn forget_program(&mut self) {
        // A check under way no longer counts.
        self.external_check += 1;
        self.settings.ffmpeg = None;
        self.external.clear();
        self.use_external = false;
        self.external_note.clear();
        self.save_settings();
        self.refresh_settings_dialog();
    }

    pub fn settings_close(&mut self) {
        self.settings_open = false;
        if let Some(window) = self.window() {
            window.set_settings_open(false);
        }
    }

    /// Writes the settings file on the file worker, saying so if it cannot.
    pub(crate) fn save_settings(&self) {
        let Some(dir) = self.settings_dir.clone() else {
            return;
        };
        let kept = self.settings.clone();
        self.files.run(move || {
            if let Err(error) = settings::write_settings(&dir, &kept) {
                let message = format!(
                    "Dusk could not save its settings in {}: {error}. They last until Dusk \
                     closes.",
                    dir.display()
                );
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| app.fail(&message));
                });
            }
        });
    }

    /// Shows the settings as they stand, while the dialog is open.
    pub(crate) fn refresh_settings_dialog(&self) {
        if !self.settings_open {
            return;
        }
        let Some(window) = self.window() else {
            return;
        };
        let settings = &self.settings;
        let export = &settings.export;
        let codecs = export.container.video_codecs();
        let number = |value: usize| i32::try_from(value).unwrap_or(i32::MAX);
        window.set_settings_view(SettingsView {
            cache: i32::try_from(settings.cache_mb).unwrap_or(i32::MAX),
            cache_minimum: i32::try_from(*CACHE_MB.start()).unwrap_or(0),
            cache_maximum: i32::try_from(*CACHE_MB.end()).unwrap_or(i32::MAX),
            containers: strings(Container::ALL.iter().map(|container| container.name())),
            container: Container::ALL
                .iter()
                .position(|container| *container == export.container)
                .map_or(-1, number),
            codecs: strings(codecs.iter().map(|codec| codec.name())),
            codec: codecs
                .iter()
                .position(|codec| *codec == export.codec)
                .map_or(-1, number),
            sizes: strings(size_names().iter().map(String::as_str)),
            size: number(size_index(settings)),
            level: i32::from(export.level),
            ffmpeg: settings
                .ffmpeg
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
                .into(),
            use_ffmpeg: settings.use_ffmpeg,
            ffmpeg_note: self.external_note.clone().into(),
        });
        window.set_settings_open(true);
    }
}

fn strings<'a>(items: impl Iterator<Item = &'a str>) -> ModelRc<SharedString> {
    let items: Vec<SharedString> = items.map(SharedString::from).collect();
    ModelRc::from(Rc::new(VecModel::from(items)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_engine::VideoCodec;

    #[test]
    fn the_cache_takes_its_range() {
        let mut settings = Settings::default();
        assert!(change(&mut settings, "cache", 1024));
        assert_eq!(settings.cache_mb, 1024);
        assert!(!change(&mut settings, "cache", 1024));
        change(&mut settings, "cache", 5);
        assert_eq!(settings.cache_mb, *CACHE_MB.start());
        change(&mut settings, "cache", 100_000);
        assert_eq!(settings.cache_mb, *CACHE_MB.end());
    }

    #[test]
    fn a_format_keeps_the_codec_when_it_holds_it() {
        let mut settings = Settings::default();
        assert!(change(&mut settings, "container", 2));
        assert_eq!(settings.export.container, Container::Mkv);
        assert_eq!(settings.export.codec, VideoCodec::H264);
        // MKV's codecs: H.264, HEVC, AV1, VP9.
        change(&mut settings, "codec", 3);
        assert_eq!(settings.export.codec, VideoCodec::Vp9);
        // WebM holds VP9 too; MP4 does not, so it takes its first.
        change(&mut settings, "container", 3);
        assert_eq!(settings.export.codec, VideoCodec::Vp9);
        change(&mut settings, "container", 0);
        assert_eq!(settings.export.codec, VideoCodec::H264);
        // Out of range, nothing changes.
        assert!(!change(&mut settings, "container", 9));
        assert!(!change(&mut settings, "codec", -1));
    }

    #[test]
    fn sizes_are_the_sequences_own_or_a_preset() {
        let mut settings = Settings::default();
        assert_eq!(
            size_names(),
            ["The sequence's own", "1080p", "720p", "480p"]
        );
        assert_eq!(size_index(&settings), 0);
        assert!(change(&mut settings, "size", 2));
        assert_eq!(settings.export.short_side, Some(720));
        assert_eq!(size_index(&settings), 2);
        change(&mut settings, "size", 0);
        assert_eq!(settings.export.short_side, None);
        assert!(!change(&mut settings, "size", 4));
        assert_eq!(PRESETS.len() + 1, size_names().len());
    }

    #[test]
    fn quality_and_the_users_ffmpeg_change() {
        let mut settings = Settings::default();
        assert!(change(&mut settings, "level", 40));
        assert_eq!(settings.export.level, 40);
        change(&mut settings, "level", 300);
        assert_eq!(settings.export.level, 100);
        assert!(change(&mut settings, "use-ffmpeg", 0));
        assert!(!settings.use_ffmpeg);
        assert!(change(&mut settings, "use-ffmpeg", 1));
        assert!(settings.use_ffmpeg);
        assert!(!change(&mut settings, "colour", 1));
    }
}
