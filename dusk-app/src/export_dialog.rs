//! The export dialog on the UI thread (docs/ARCHITECTURE.md, "Export details"): it opens
//! over the main window for the timeline or over the clip editor for its clip, probes the
//! encoders on a worker the first time, asks where to save with the system's dialog and
//! starts the export. What it offers is worked out in `export_choices`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dusk_core::{Frame, Project};
use dusk_engine::{
    AudioCodec, AudioFormat, Encoder, EngineError, ExportFormat, ExportSettings,
    available_encoders, has_picture, has_sound,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::ExportView;
use crate::app::{App, with_app};
use crate::export_choices::ExportChoices;
use crate::files::{clip_export_path, export_path, same_file};
use crate::platform::Dialog;

/// What an export writes.
pub enum ExportTarget {
    /// The timeline, from the main window.
    Timeline,
    /// The clip editor's clip alone: its export project, and the source it comes from.
    Clip {
        project: Box<Project>,
        source: PathBuf,
    },
}

/// The export dialog while it is open.
pub struct ExportDialog {
    target: ExportTarget,
    /// What it offers and what is chosen; `None` until the encoders have been tried.
    choices: Option<ExportChoices>,
    /// The picture size and what there is, for the choices once the encoders are known.
    size: (u32, u32),
    has_video: bool,
    has_sound: bool,
}

impl ExportDialog {
    fn in_editor(&self) -> bool {
        matches!(self.target, ExportTarget::Clip { .. })
    }

    /// Whether Export can go ahead: the encoders are known and what is chosen can be made.
    fn ready(&self) -> bool {
        self.choices.as_ref().is_some_and(ExportChoices::ready)
    }
}

impl App {
    /// Ctrl+E, the toolbar and the File menu: the export dialog for the timeline.
    pub(crate) fn export(&mut self) {
        if self.export.is_some() {
            return self.say("An export is already running.");
        }
        if self.project.sequence().end() == Frame(0) {
            return self.fail("Place some media on the timeline before exporting.");
        }
        self.open_export_dialog(ExportTarget::Timeline);
    }

    /// Opens the export dialog for `target`; the first time, the encoders are tried on a
    /// worker meanwhile.
    pub(crate) fn open_export_dialog(&mut self, target: ExportTarget) {
        if self.export_dialog.is_some() {
            return;
        }
        let project = match &target {
            ExportTarget::Timeline => &*self.project,
            ExportTarget::Clip { project, .. } => project,
        };
        let mut dialog = ExportDialog {
            size: project.sequence().resolution(),
            has_video: has_picture(project),
            has_sound: has_sound(project),
            target,
            choices: None,
        };
        match &self.encoders {
            Some(encoders) => dialog.choices = Some(self.choices_for(&dialog, encoders.clone())),
            None if !self.probing_encoders => {
                self.probing_encoders = true;
                let spawned = std::thread::Builder::new()
                    .name("dusk encoder probe".to_owned())
                    .spawn(|| {
                        let found = available_encoders().to_vec();
                        let _ = slint::invoke_from_event_loop(move || {
                            with_app(|app| app.encoders_probed(found));
                        });
                    });
                if let Err(error) = spawned {
                    self.probing_encoders = false;
                    return self.fail(&EngineError::Thread(error).to_string());
                }
            }
            None => {}
        }
        self.export_dialog = Some(dialog);
        self.refresh_export_dialog();
    }

    fn choices_for(&self, dialog: &ExportDialog, encoders: Vec<&'static Encoder>) -> ExportChoices {
        ExportChoices::new(
            encoders,
            dialog.size,
            dialog.has_video,
            dialog.has_sound,
            self.last_export,
        )
    }

    /// The encoders that open on this machine are known.
    fn encoders_probed(&mut self, encoders: Vec<&'static Encoder>) {
        self.probing_encoders = false;
        self.encoders = Some(encoders.clone());
        if let Some(dialog) = self.export_dialog.take() {
            let choices = self.choices_for(&dialog, encoders);
            self.export_dialog = Some(ExportDialog {
                choices: Some(choices),
                ..dialog
            });
        }
        self.refresh_export_dialog();
    }

    /// The dialog's `what` changed to `value` (see ui/export-dialog.slint).
    pub fn export_changed(&mut self, what: &str, value: i32) {
        let Some(choices) = self
            .export_dialog
            .as_mut()
            .and_then(|dialog| dialog.choices.as_mut())
        else {
            return;
        };
        let index = usize::try_from(value).unwrap_or(usize::MAX);
        match what {
            "kind" => choices.sound_only = value == 1,
            "container" => {
                if let Some(container) = choices.containers().get(index) {
                    choices.set_container(*container);
                }
            }
            "codec" => {
                if let Some(codec) = choices.codecs().get(index) {
                    choices.codec = *codec;
                }
            }
            "sound" => {
                if let Some(audio) = choices.container.audio_codecs().get(index) {
                    choices.audio = *audio;
                }
            }
            "sound-format" => {
                if let Some(format) = AudioFormat::ALL.get(index) {
                    choices.sound_format = *format;
                }
            }
            "size" => {
                if let Some(size) = choices.sizes().get(index) {
                    choices.short_side = *size;
                }
            }
            "level" => choices.level = value.clamp(0, 100) as u8,
            "bitrate" => choices.bitrate_kbps = u32::try_from(value).ok().filter(|kbps| *kbps > 0),
            "crf" => choices.crf = u8::try_from(value.min(63)).ok().filter(|crf| *crf > 0),
            _ => {}
        }
        self.refresh_export_dialog();
    }

    /// The dialog closed: on Export, ask where and start; on Cancel, nothing.
    pub fn export_done(&mut self, export: bool) {
        let Some(dialog) = self.export_dialog.take() else {
            return;
        };
        let in_editor = dialog.in_editor();
        self.hide_export_dialog(in_editor);
        let (true, Some(choices)) = (export, &dialog.choices) else {
            return;
        };
        let settings = choices.settings();
        self.last_export = Some(settings);
        let extension = settings.extension();
        let suggested = match &dialog.target {
            ExportTarget::Timeline => {
                let base = match &self.document.path {
                    Some(path) => path.clone(),
                    None => match self.project.media().first() {
                        Some(media) => media.path.clone(),
                        None => return self.fail("Import some media before exporting."),
                    },
                };
                export_path(&base, extension)
            }
            ExportTarget::Clip { source, .. } => {
                // Beside the project file once it has one, otherwise beside the source.
                let folder = self
                    .document
                    .path
                    .as_deref()
                    .or(Some(source.as_path()))
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                clip_export_path(&folder, source, extension)
            }
        };
        let ask = Dialog::Export {
            suggested,
            kind: kind_name(&settings),
        };
        let target = dialog.target;
        let done = move |paths: Vec<PathBuf>| {
            if let Some(path) = paths.into_iter().next() {
                with_app(|app| app.start_export(target, path, settings));
            }
        };
        if in_editor {
            if let Some(window) = self
                .editor_window
                .as_ref()
                .map(ComponentHandle::clone_strong)
            {
                self.show_dialog_over(window.window(), ask, done);
            }
        } else if let Some(window) = self.window() {
            self.show_dialog_over(window.window(), ask, done);
        }
    }

    /// Writes `target` to `path` with `settings`; never over a media file of the project.
    fn start_export(&mut self, target: ExportTarget, path: PathBuf, settings: ExportSettings) {
        let in_editor = matches!(target, ExportTarget::Clip { .. });
        let fail = |app: &App, message: &str| {
            if in_editor {
                app.editor_fail(message);
            } else {
                app.fail(message);
            }
        };
        if self.export.is_some() {
            return fail(self, "An export is already running.");
        }
        if self
            .project
            .media()
            .iter()
            .any(|media| same_file(&media.path, &path))
        {
            return fail(
                self,
                "That file is used in the project, and Dusk never writes over media. Export \
                 under another name.",
            );
        }
        let project = match target {
            ExportTarget::Timeline => Arc::clone(&self.project),
            ExportTarget::Clip { project, .. } => Arc::new(*project),
        };
        match self.engine.export(project, path.clone(), settings) {
            Ok(job) => {
                self.export_started(job, in_editor);
                let what = if in_editor { "the clip " } else { "" };
                let message = format!("Exporting {what}to {}…", path.display());
                self.say(&message);
                if in_editor {
                    self.editor_say(&message);
                }
            }
            Err(error) => fail(self, &crate::app::sentence(&error.to_string())),
        }
    }

    /// Escape cancels the dialog and Enter exports once it can; true when the dialog is open
    /// in the window (`in_editor`) the key was pressed in, which then takes every key.
    pub(crate) fn export_dialog_key(&mut self, text: &str, in_editor: bool) -> bool {
        use slint::platform::Key;
        let Some(dialog) = &self.export_dialog else {
            return false;
        };
        if dialog.in_editor() != in_editor {
            return false;
        }
        if text == SharedString::from(Key::Escape).as_str() {
            self.export_done(false);
        } else if text == SharedString::from(Key::Return).as_str() && dialog.ready() {
            self.export_done(true);
        }
        true
    }

    /// Closes the dialog without exporting, as when its window closes.
    pub(crate) fn cancel_export_dialog(&mut self) {
        if self.export_dialog.is_some() {
            self.export_done(false);
        }
    }

    /// Whether the export dialog is open over the clip editor.
    pub(crate) fn export_dialog_in_editor(&self) -> bool {
        self.export_dialog
            .as_ref()
            .is_some_and(ExportDialog::in_editor)
    }

    fn hide_export_dialog(&self, in_editor: bool) {
        if in_editor {
            if let Some(window) = &self.editor_window {
                window.set_export_open(false);
                window.invoke_take_keys();
            }
        } else if let Some(window) = self.window() {
            window.set_export_open(false);
            window.invoke_take_keys();
        }
    }

    /// Shows the dialog as the choices stand.
    fn refresh_export_dialog(&self) {
        let Some(dialog) = &self.export_dialog else {
            return;
        };
        let view = export_view(dialog);
        if dialog.in_editor() {
            if let Some(window) = &self.editor_window {
                window.set_export_view(view);
                window.set_export_open(true);
            }
        } else if let Some(window) = self.window() {
            window.set_export_view(view);
            window.set_export_open(true);
        }
    }
}

/// The format's name for the save dialog's filter, such as "MP4 video".
fn kind_name(settings: &ExportSettings) -> String {
    match settings.format {
        ExportFormat::Video { container, .. } => format!("{} video", container.name()),
        ExportFormat::Sound(format) => format!("{} sound", format.name()),
    }
}

fn audio_name(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Aac => "AAC",
        AudioCodec::Opus => "Opus",
        AudioCodec::Mp3 => "MP3",
        AudioCodec::Pcm => "WAV",
    }
}

fn strings(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    let items: Vec<SharedString> = items.into_iter().map(SharedString::from).collect();
    ModelRc::from(std::rc::Rc::new(VecModel::from(items)))
}

fn index_of<T: PartialEq>(items: &[T], item: &T) -> i32 {
    items
        .iter()
        .position(|candidate| candidate == item)
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(-1)
}

/// What the dialog shows of `dialog`.
fn export_view(dialog: &ExportDialog) -> ExportView {
    let Some(choices) = &dialog.choices else {
        return ExportView {
            probing: true,
            ..ExportView::default()
        };
    };
    let ready = dialog.ready();
    let containers = choices.containers();
    let codecs = choices.codecs();
    let sounds = choices.container.audio_codecs();
    let sizes = choices.sizes();
    let encoder = choices.encoder().map_or_else(String::new, |encoder| {
        if encoder.hardware {
            format!("On the graphics card ({})", encoder.name)
        } else {
            format!(
                "In software ({}); slower than a graphics card",
                encoder.name
            )
        }
    });
    ExportView {
        probing: false,
        ready,
        video_offered: choices.video_offered(),
        sound_offered: choices.sound_offered(),
        sound_only: choices.sound_only,
        containers: strings(containers.iter().map(|c| c.name().to_owned())),
        container: index_of(&containers, &choices.container),
        codecs: strings(codecs.iter().map(|c| c.name().to_owned())),
        codec: index_of(&codecs, &choices.codec),
        encoder: encoder.into(),
        sounds: strings(sounds.iter().map(|codec| audio_name(*codec).to_owned())),
        sound: index_of(sounds, &choices.audio),
        sound_formats: strings(AudioFormat::ALL.iter().map(|f| f.name().to_owned())),
        sound_format: index_of(&AudioFormat::ALL, &choices.sound_format),
        sizes: strings(sizes.iter().map(|size| {
            let (width, height) = choices.size_of(*size);
            format!("{width} × {height}")
        })),
        size: index_of(&sizes, &choices.short_side),
        level: i32::from(choices.level),
        crf_offered: choices.crf_offered(),
        bitrate: choices
            .bitrate_kbps
            .and_then(|kbps| i32::try_from(kbps).ok())
            .unwrap_or(0),
        crf: choices.crf.map_or(0, i32::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_engine::ENCODERS;

    fn dialog(choices: Option<ExportChoices>) -> ExportDialog {
        ExportDialog {
            target: ExportTarget::Timeline,
            choices,
            size: (1920, 1080),
            has_video: true,
            has_sound: true,
        }
    }

    #[test]
    fn nothing_can_be_exported_while_the_encoders_are_tried() {
        assert!(!dialog(None).ready());
        let choices = ExportChoices::new(ENCODERS.iter().collect(), (1920, 1080), true, true, None);
        assert!(dialog(Some(choices)).ready());
    }
}
