//! The export dialog on the UI thread (docs/ARCHITECTURE.md, "Export details"): it opens
//! over the main window for the timeline or over the clip editor for its clip, probes the
//! encoders on a worker the first time, asks where to save with the system's dialog and
//! starts the export. What it offers is worked out in `export_choices`. Its Advanced section
//! takes the user's own `ffmpeg` for the GPL encoders ("Optional GPL encoders"), checked on a
//! worker and kept for the session.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dusk_core::{Frame, Project};
use dusk_engine::{
    AudioCodec, AudioFormat, Encoder, EngineError, ExportFormat, ExportSettings, ExternalEncoder,
    available_encoders, external_encoders, has_picture, has_sound,
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
        let mut choices = ExportChoices::new(
            encoders,
            dialog.size,
            dialog.has_video,
            dialog.has_sound,
            self.last_export,
        );
        choices.external = self.external.clone();
        choices.use_external = self.use_external;
        choices
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
        if what == "program" {
            return self.pick_program();
        }
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
            "external" => choices.use_external = value == 1,
            _ => {}
        }
        self.refresh_export_dialog();
    }

    /// Asks for the user's own `ffmpeg` program with the system's dialog, over the export
    /// dialog's window, and checks the program picked.
    fn pick_program(&mut self) {
        let Some(dialog) = &self.export_dialog else {
            return;
        };
        let done = |paths: Vec<PathBuf>| {
            if let Some(program) = paths.into_iter().next() {
                with_app(|app| app.check_program(program));
            }
        };
        if dialog.in_editor() {
            if let Some(window) = self
                .editor_window
                .as_ref()
                .map(ComponentHandle::clone_strong)
            {
                self.show_dialog_over(window.window(), Dialog::OpenProgram, done);
            }
        } else if let Some(window) = self.window() {
            self.show_dialog_over(window.window(), Dialog::OpenProgram, done);
        }
    }

    /// Asks `program` for its encoders on a worker; the dialog says so meanwhile.
    fn check_program(&mut self, program: PathBuf) {
        self.external_note = format!("Checking {}…", program.display());
        self.refresh_export_dialog();
        let spawned = std::thread::Builder::new()
            .name("dusk ffmpeg check".to_owned())
            .spawn(move || {
                let checked = external_encoders(&program);
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| app.program_checked(&program, checked));
                });
            });
        if let Err(error) = spawned {
            self.external_note = crate::app::sentence(&EngineError::Thread(error).to_string());
            self.refresh_export_dialog();
        }
    }

    /// What `program` has is known: exports use it from now on when it has a GPL encoder.
    fn program_checked(
        &mut self,
        program: &Path,
        checked: Result<Vec<ExternalEncoder>, EngineError>,
    ) {
        match checked {
            Ok(found) => {
                self.external_note = program_note(program, &found);
                self.use_external = !found.is_empty();
                self.external = found;
            }
            Err(error) => {
                self.external_note = crate::app::sentence(&error.to_string());
                self.use_external = false;
                self.external.clear();
            }
        }
        if let Some(choices) = self
            .export_dialog
            .as_mut()
            .and_then(|dialog| dialog.choices.as_mut())
        {
            choices.external = self.external.clone();
            choices.use_external = self.use_external;
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
        self.use_external = choices.use_external;
        let external = choices.chosen_external().cloned();
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
                with_app(|app| app.start_export(target, path, settings, external));
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

    /// Writes `target` to `path` with `settings`, the video through the user's own `ffmpeg`
    /// when `external` is given; never over a media file of the project.
    fn start_export(
        &mut self,
        target: ExportTarget,
        path: PathBuf,
        settings: ExportSettings,
        external: Option<ExternalEncoder>,
    ) {
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
        let started = match external {
            Some(external) => {
                self.engine
                    .export_external(project, path.clone(), settings, external)
            }
            None => self.engine.export(project, path.clone(), settings),
        };
        match started {
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
        let view = export_view(dialog, &self.external_note);
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

/// What the dialog says of the user's `program`, which has the GPL encoders `found`.
fn program_note(program: &Path, found: &[ExternalEncoder]) -> String {
    let names: Vec<&str> = found.iter().map(|external| external.encoder.name).collect();
    match names.as_slice() {
        [] => format!(
            "{} has neither libx264 nor libx265, so Dusk's own encoders are used.",
            program.display()
        ),
        [one] => format!("{} has {one}.", program.display()),
        [first @ .., last] => format!("{} has {} and {last}.", program.display(), first.join(", ")),
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

/// What the dialog shows of `dialog`, with `external_note` about the user's own `ffmpeg`.
fn export_view(dialog: &ExportDialog, external_note: &str) -> ExportView {
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
        if choices.chosen_external().is_some() {
            format!("In your ffmpeg ({})", encoder.name)
        } else if encoder.hardware {
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
        external_offered: choices.external_offered(),
        external: choices.chosen_external().is_some(),
        external_note: external_note.into(),
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

    #[test]
    fn the_note_names_the_gpl_encoders_the_program_has() {
        let program = Path::new("C:/tools/ffmpeg.exe");
        let with = |names: &[&str]| -> Vec<ExternalEncoder> {
            names
                .iter()
                .map(|name| ExternalEncoder {
                    program: program.to_path_buf(),
                    encoder: dusk_engine::external_encoder_named(name).unwrap(),
                })
                .collect()
        };
        assert_eq!(
            program_note(program, &with(&[])),
            "C:/tools/ffmpeg.exe has neither libx264 nor libx265, so Dusk's own encoders are used."
        );
        assert_eq!(
            program_note(program, &with(&["libx264"])),
            "C:/tools/ffmpeg.exe has libx264."
        );
        assert_eq!(
            program_note(program, &with(&["libx264", "libx265"])),
            "C:/tools/ffmpeg.exe has libx264 and libx265."
        );
    }

    #[test]
    fn the_dialog_says_when_the_users_ffmpeg_writes_the_video() {
        let mut choices =
            ExportChoices::new(ENCODERS.iter().collect(), (1920, 1080), true, true, None);
        let note = "Your ffmpeg has libx264.";
        let view = export_view(&dialog(Some(choices.clone())), note);
        assert!(!view.external_offered);
        assert_eq!(view.external_note, note);
        choices.external = vec![dusk_engine::ExternalEncoder {
            program: "ffmpeg.exe".into(),
            encoder: dusk_engine::external_encoder_named("libx264").unwrap(),
        }];
        choices.use_external = true;
        let view = export_view(&dialog(Some(choices.clone())), note);
        assert!(view.external_offered && view.external);
        assert_eq!(view.encoder, "In your ffmpeg (libx264)");
        assert!(view.crf_offered);
        choices.use_external = false;
        let view = export_view(&dialog(Some(choices)), note);
        assert!(view.external_offered && !view.external);
        assert_eq!(view.encoder, "On the graphics card (h264_nvenc)");
    }
}
