//! The pop-out clip editor (docs/ARCHITECTURE.md, "Pop-out clip editor"): it opens a clip
//! with the clips linked to it, previews the draft through the engine's second preview with
//! a transport of its own, takes the window's changes to the draft, and applies the draft to
//! the project as one undoable step. What each change does to the draft is in `draft`; the
//! session and its command are in `dusk-core`.

use std::sync::Arc;

use dusk_core::{
    ClipDraft, ClipEditSession, ClipId, Edge, Fit, Frame, MediaTime, Project, Rational,
    SessionStatus, VideoEdits,
};
use dusk_engine::Preview;
use slint::{CloseRequestResponse, ComponentHandle, Model, SharedString};

use crate::app::{App, factor_label, sentence, texture_image, with_app};
use crate::document::Next;
use crate::draft::{self, EditorView, Sides, Source};
use crate::export_dialog::ExportTarget;
use crate::navigation::one_second;
use crate::platform;
use crate::shortcuts::Action;
use crate::speed::{SpeedKey, next_factor};
use crate::timeline::timecode;
use crate::{ClipEditorWindow, EditorProps, EditorSides, StatusKind};

/// The clip editor's work while its window is open.
pub struct ClipEditor {
    session: ClipEditSession,
    /// The frame the preview shows, counted from the clip's start.
    playhead: Frame,
    /// What the preview shows: the clips alone with the draft applied.
    preview: Arc<Project>,
    /// The question on the window, if one is, and whether it offers to apply the draft.
    question: Option<(AfterDraft, bool)>,
}

impl ClipEditor {
    fn last_frame(&self) -> Frame {
        (self.preview.sequence().end() - Frame(1)).max(Frame(0))
    }
}

/// What comes after the question about a draft that was not applied.
pub enum AfterDraft {
    /// The clip editor closes.
    Close,
    /// This clip opens in it instead.
    Open(ClipId),
    /// The clip editor closes and the main window goes on with this.
    Then(Next),
}

/// Where the playhead goes after a change to the draft.
#[derive(Clone, Copy)]
enum PlayheadTo {
    /// It stays, or comes back inside the clip.
    Same,
    First,
    Last,
}

impl App {
    /// A clip was double-clicked on the timeline.
    pub fn open_clip_id(&mut self, clip: i32) {
        if let Ok(id) = u64::try_from(clip) {
            self.open_clip(ClipId(id));
        }
    }

    /// Enter, the Edit menu or the properties panel: opens the selected clip.
    pub(crate) fn open_selected_clip(&mut self) {
        match self.selected_clip {
            Some(clip) => self.open_clip(clip),
            None => self.say("Select a clip to open it in the clip editor."),
        }
    }

    /// Opens `clip` and the clips linked to it in the clip editor. Changes to other clips
    /// that were not applied are asked about first.
    pub(crate) fn open_clip(&mut self, clip: ClipId) {
        if let Some(editor) = &self.editor {
            if editor.session.group().contains(&clip) {
                return self.raise_editor();
            }
            if editor.session.changed() {
                return self.ask_about_draft(AfterDraft::Open(clip));
            }
        }
        self.open_clip_now(clip);
    }

    fn open_clip_now(&mut self, clip: ClipId) {
        let session = match ClipEditSession::open(&self.project, clip) {
            Ok(session) => session,
            Err(rejection) => return self.fail(&sentence(&rejection.to_string())),
        };
        let preview = match session.preview_project(&self.project) {
            Ok(preview) => Arc::new(preview),
            Err(rejection) => return self.fail(&sentence(&rejection.to_string())),
        };
        let Some(window) = self.editor_window() else {
            return;
        };
        self.stop_playing(Preview::ClipEditor);
        self.engine
            .set_project(Preview::ClipEditor, Arc::clone(&preview));
        self.engine.show(Preview::ClipEditor, Frame(0));
        self.editor = Some(ClipEditor {
            session,
            playhead: Frame(0),
            preview,
            question: None,
        });
        window.set_prompt_open(false);
        window.set_preview_image(slint::Image::default());
        self.editor_say("");
        self.refresh_editor();
        // Showing a window that is shown already would set up its drawing surface again.
        if !window.window().is_visible()
            && let Err(error) = window.show()
        {
            self.forget_editor();
            return self.fail(&format!("The clip editor could not open: {error}."));
        }
        // Showing the window sized its preview, but the callback came too early to be heard.
        self.editor_preview_resized(
            window.get_preview_pixel_width(),
            window.get_preview_pixel_height(),
        );
        self.raise_editor();
        window.invoke_take_keys();
    }

    /// The clip editor's window, made the first time it is needed and kept for the next.
    fn editor_window(&mut self) -> Option<ClipEditorWindow> {
        if self.editor_window.is_none() {
            match ClipEditorWindow::new() {
                Ok(window) => {
                    connect(&window);
                    self.editor_window = Some(window);
                }
                Err(error) => {
                    self.fail(&format!("The clip editor could not open: {error}."));
                    return None;
                }
            }
        }
        self.editor_window
            .as_ref()
            .map(ComponentHandle::clone_strong)
    }

    fn raise_editor(&self) {
        if let Some(window) = &self.editor_window {
            platform::bring_to_front(window.window());
        }
    }

    /// Whether the clip editor holds changes that are not in the project.
    pub(crate) fn draft_pending(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor.session.changed())
    }

    /// Ctrl+W: closes the clip editor, asking first about changes not applied.
    pub(crate) fn close_clip_editor(&mut self) {
        if self.draft_pending() {
            return self.ask_about_draft(AfterDraft::Close);
        }
        self.close_editor_now();
    }

    /// Closes the clip editor; its draft is let go.
    pub(crate) fn close_editor_now(&mut self) {
        self.forget_editor();
        if let Some(window) = &self.editor_window {
            let _ = window.hide();
        }
    }

    /// The system's close button on the clip editor: true when it may close now. With
    /// changes not applied it asks first, and closes once they are applied or let go.
    pub fn may_close_editor(&mut self) -> bool {
        if self.draft_pending() {
            self.ask_about_draft(AfterDraft::Close);
            return false;
        }
        self.forget_editor();
        true
    }

    /// Lets go of the clip editor's session and preview; whoever closes the window hides it.
    fn forget_editor(&mut self) {
        if self.export_dialog_in_editor() {
            self.cancel_export_dialog();
        }
        if self.editor.take().is_some() {
            self.engine.close_preview(Preview::ClipEditor);
        }
        if let Some(window) = &self.editor_window {
            window.set_preview_image(slint::Image::default());
            window.set_prompt_open(false);
        }
    }

    /// Asks in the clip editor what to do with changes not applied before `then`: apply
    /// them, let them go, or stay.
    pub(crate) fn ask_about_draft(&mut self, then: AfterDraft) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        if editor.question.is_some() {
            return self.raise_editor();
        }
        let view = draft::editor_view(&self.project, &editor.session);
        let applicable =
            editor.session.status(&self.project) != SessionStatus::Broken && !view.locked;
        let name = view.name;
        let (title, message, answers, destructive): (String, &str, &[&str], usize) = if applicable {
            (
                format!("Apply the changes to {name}?"),
                "Changes you don't apply are lost.",
                &["Apply", "Discard", "Cancel"],
                1,
            )
        } else {
            (
                format!("Discard the changes to {name}?"),
                "They can't be applied: the clip was deleted or unlinked, or its track is locked.",
                &["Discard", "Cancel"],
                0,
            )
        };
        if let Some(editor) = self.editor.as_mut() {
            editor.question = Some((then, applicable));
        }
        if let Some(window) = &self.editor_window {
            let answers: Vec<SharedString> =
                answers.iter().map(|answer| (*answer).into()).collect();
            window.set_prompt_title(title.into());
            window.set_prompt_message(message.into());
            window.set_prompt_answers(std::rc::Rc::new(slint::VecModel::from(answers)).into());
            window.set_prompt_destructive(i32::try_from(destructive).unwrap_or(-1));
            window.set_prompt_open(true);
        }
        self.raise_editor();
    }

    /// The answer at `index` was given to the clip editor's question.
    pub fn editor_answer(&mut self, index: usize) {
        let Some((then, applicable)) = self
            .editor
            .as_mut()
            .and_then(|editor| editor.question.take())
        else {
            return;
        };
        if let Some(window) = &self.editor_window {
            window.set_prompt_open(false);
            window.invoke_take_keys();
        }
        // Apply, Discard, Cancel; or, when the draft cannot be applied, Discard, Cancel.
        let answer = if applicable {
            index
        } else {
            index.saturating_add(1)
        };
        match answer {
            0 if !self.apply_draft() => return,
            0 | 1 => {}
            _ => return,
        }
        match then {
            AfterDraft::Close => self.close_editor_now(),
            AfterDraft::Open(clip) => self.open_clip_now(clip),
            AfterDraft::Then(next) => {
                self.close_editor_now();
                self.after_unsaved_changes(next);
            }
        }
    }

    /// Enter gives the question's first answer, Escape its last.
    fn editor_answer_with_key(&mut self, text: &str) {
        use slint::platform::Key;
        let Some(window) = &self.editor_window else {
            return;
        };
        let last = window.get_prompt_answers().row_count().saturating_sub(1);
        if text == SharedString::from(Key::Return).as_str() {
            self.editor_answer(0);
        } else if text == SharedString::from(Key::Escape).as_str() {
            self.editor_answer(last);
        }
    }

    /// Apply to project: the draft becomes one undoable edit of its clips. True when it was
    /// applied, or there was nothing to apply.
    pub(crate) fn apply_draft(&mut self) -> bool {
        let Some(editor) = &self.editor else {
            return false;
        };
        if !editor.session.changed() {
            return true;
        }
        let command = editor.session.apply_command();
        match self.try_edit(command) {
            Ok(notices) => {
                // The clips as applied: a cut at the gap makes them shorter than drafted.
                if let Some(editor) = self.editor.as_mut() {
                    let _ = editor.session.reload(&self.project);
                }
                self.editor_follow_project();
                let message = format!("Applied to the project. {notices}");
                let message = message.trim_end();
                if notices.is_empty() {
                    self.say(message);
                    self.editor_say(message);
                } else {
                    self.warn(message);
                    self.editor_warn(message);
                }
                true
            }
            Err(reason) => {
                self.fail(&reason);
                self.editor_fail(&reason);
                false
            }
        }
    }

    /// Reload from project: the clip changed in the main window, and the draft starts again
    /// from it as it is now.
    fn editor_reload(&mut self) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if editor.session.status(&self.project) != SessionStatus::Edited {
            return;
        }
        if let Err(rejection) = editor.session.reload(&self.project) {
            return self.editor_fail(&sentence(&rejection.to_string()));
        }
        self.editor_say("The clip is as it is now in the project.");
        self.editor_follow_project();
    }

    /// Keep my draft: the clip changed in the main window, and the draft stays, to be applied
    /// over the clip as it is now.
    fn editor_keep_draft(&mut self) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if editor.session.status(&self.project) != SessionStatus::Edited {
            return;
        }
        editor.session.keep_draft(&self.project);
        self.editor_say(
            "Your draft stays; applying it replaces the changes made in the main window.",
        );
        self.refresh_editor();
    }

    /// The project changed: the clip editor's preview follows the sequence's rate and size,
    /// and its window shows how its clips stand now. A draft without changes of its own
    /// simply takes the clips as they are now.
    pub(crate) fn editor_follow_project(&mut self) {
        let playing = self.playing(Preview::ClipEditor).is_some();
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let reloaded = !editor.session.changed()
            && editor.session.status(&self.project) == SessionStatus::Edited
            && editor.session.reload(&self.project).is_ok();
        // What the status line said was about the clips as they were.
        let stale = reloaded || editor.session.status(&self.project) != SessionStatus::Current;
        if let Ok(preview) = editor.session.preview_project(&self.project)
            && preview != *editor.preview
        {
            editor.preview = Arc::new(preview);
            editor.playhead = editor.playhead.min(editor.last_frame());
            self.engine
                .set_project(Preview::ClipEditor, Arc::clone(&editor.preview));
            if !playing {
                self.engine.show(Preview::ClipEditor, editor.playhead);
            }
        }
        if stale {
            self.editor_say("");
        }
        self.refresh_editor();
    }

    /// Changes the draft with `change`. The preview shows the result; or, when a rule of the
    /// timeline refuses it, the window says why and the draft stays as it was. `exact`
    /// shows that very frame, otherwise the nearest one at hand, as while dragging.
    fn change_draft(
        &mut self,
        to: PlayheadTo,
        exact: bool,
        change: impl FnOnce(&mut ClipDraft, &Source, Rational),
    ) {
        let playing = self.playing(Preview::ClipEditor).is_some();
        let rate = self.project.sequence().frame_rate();
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let Some(source) = Source::of(&self.project, &editor.session) else {
            return;
        };
        let mut session = editor.session.clone();
        change(&mut session.draft, &source, rate);
        let outcome = if session.draft == editor.session.draft {
            Ok(())
        } else {
            session.preview_project(&self.project).map(|preview| {
                editor.session = session;
                editor.preview = Arc::new(preview);
                self.engine
                    .set_project(Preview::ClipEditor, Arc::clone(&editor.preview));
            })
        };
        if !playing {
            editor.playhead = match to {
                PlayheadTo::Same => editor.playhead.min(editor.last_frame()),
                PlayheadTo::First => Frame(0),
                PlayheadTo::Last => editor.last_frame(),
            };
            if exact {
                self.engine.show(Preview::ClipEditor, editor.playhead);
            } else {
                self.engine.scrub(Preview::ClipEditor, editor.playhead);
            }
        }
        match outcome {
            Ok(()) => self.editor_say(""),
            Err(rejection) => self.editor_fail(&sentence(&rejection.to_string())),
        }
        // A refused value goes back to what the draft has.
        self.refresh_editor();
    }

    /// Changes the draft's picture edits; a clip without a picture has none to change.
    fn change_picture(&mut self, change: impl FnOnce(&mut VideoEdits, &Source)) {
        self.change_draft(PlayheadTo::Same, true, |draft, source, _| {
            if let Some(edits) = draft.video.as_mut() {
                change(edits, source);
            }
        });
    }

    /// Changes the draft's sound edits.
    fn change_sound(&mut self, change: impl FnOnce(&mut dusk_core::AudioEdits)) {
        self.change_draft(PlayheadTo::Same, true, |draft, _, _| {
            if let Some(edits) = draft.audio.as_mut() {
                change(edits);
            }
        });
    }

    /// A trim handle was dragged to `at`, a fraction of the source; `done` when it was let
    /// go. The preview shows the frame at that end.
    pub fn editor_trim(&mut self, start: bool, at: f32, done: bool) {
        self.stop_playing(Preview::ClipEditor);
        let (edge, to) = if start {
            (Edge::Start, PlayheadTo::First)
        } else {
            (Edge::End, PlayheadTo::Last)
        };
        self.change_draft(to, done, |draft, source, rate| {
            let at = MediaTime((f64::from(at) * source.duration.0 as f64).round() as i64);
            draft::trim(draft, edge, at, source);
            draft::fit_fades(draft, source, rate);
        });
    }

    /// I and O: starts the clip at the frame under the playhead, or ends it there.
    fn editor_mark(&mut self, edge: Edge) {
        let Some(playhead) = self.editor.as_ref().map(|editor| editor.playhead) else {
            return;
        };
        let playhead = self.stop_playing(Preview::ClipEditor).unwrap_or(playhead);
        let to = match edge {
            Edge::Start => PlayheadTo::First,
            Edge::End => PlayheadTo::Last,
        };
        self.change_draft(to, true, |draft, source, rate| {
            if !source.still {
                draft::mark(draft, edge, playhead, rate, source);
                draft::fit_fades(draft, source, rate);
            }
        });
    }

    pub fn editor_set_speed(&mut self, percent: i32) {
        self.change_draft(PlayheadTo::Same, true, |draft, source, rate| {
            draft.speed = f64::from(percent) / 100.0;
            draft::fit_fades(draft, source, rate);
        });
    }

    /// Gives the photo a length of `frames`.
    pub fn editor_set_length(&mut self, frames: i32) {
        self.change_draft(PlayheadTo::Same, true, |draft, _, _| {
            draft.still_length = Frame(frames.into());
        });
    }

    pub fn editor_turn(&mut self, clockwise: bool) {
        self.change_picture(|edits, _| draft::turn(edits, clockwise));
    }

    /// Mirrors the picture left to right (`horizontal`) or top to bottom.
    pub fn editor_flip(&mut self, horizontal: bool) {
        self.change_picture(|edits, _| {
            if horizontal {
                edits.flip_h = !edits.flip_h;
            } else {
                edits.flip_v = !edits.flip_v;
            }
        });
    }

    pub fn editor_set_fill(&mut self, fill: bool) {
        self.change_picture(|edits, _| {
            edits.fit = if fill { Fit::Fill } else { Fit::Fit };
        });
    }

    fn editor_toggle_fill(&mut self) {
        self.change_picture(|edits, _| {
            edits.fit = match edits.fit {
                Fit::Fit => Fit::Fill,
                Fit::Fill => Fit::Fit,
            };
        });
    }

    /// Crops the picture by `sides`, as it shows.
    pub fn editor_set_crop(&mut self, sides: EditorSides) {
        let side = |pixels: i32| u32::try_from(pixels).unwrap_or(0);
        let sides = Sides {
            left: side(sides.left),
            top: side(sides.top),
            right: side(sides.right),
            bottom: side(sides.bottom),
        };
        self.change_picture(|edits, source| draft::set_shown_sides(edits, source.size, sides));
    }

    pub fn editor_set_volume(&mut self, decibels: f32) {
        self.change_sound(|edits| edits.volume_db = decibels);
    }

    pub fn editor_set_fades(&mut self, fade_in: i32, fade_out: i32) {
        self.change_sound(|edits| {
            edits.fade_in = Frame(fade_in.into());
            edits.fade_out = Frame(fade_out.into());
        });
    }

    pub fn editor_play_pause(&mut self) {
        if self.playing(Preview::ClipEditor).is_some() {
            self.editor_pause();
        } else {
            self.editor_play(1.0);
        }
    }

    /// J, L and Shift with them: plays at the speed the key goes to from the current one.
    fn editor_play_key(&mut self, key: SpeedKey) {
        self.editor_play(next_factor(self.playing(Preview::ClipEditor), key));
    }

    /// Plays the draft at `factor` from where playback is, or from the playhead; playing in
    /// the main window stops.
    fn editor_play(&mut self, factor: f64) {
        let Some((playhead, last)) = self
            .editor
            .as_ref()
            .map(|editor| (editor.playhead, editor.last_frame()))
        else {
            return;
        };
        let from = self.stop_playing(Preview::ClipEditor).unwrap_or(playhead);
        let from = if factor > 0.0 && from >= last {
            Frame(0)
        } else if factor < 0.0 && from <= Frame(0) {
            last
        } else {
            from
        };
        if let Some(editor) = self.editor.as_mut() {
            editor.playhead = from;
        }
        self.engine.play(Preview::ClipEditor, from, factor);
        self.refresh_transport();
        self.refresh_editor_transport();
    }

    fn editor_pause(&mut self) {
        self.stop_playing(Preview::ClipEditor);
        self.refresh_editor_transport();
    }

    fn editor_step(&mut self, frames: i64) {
        let Some(playhead) = self.editor.as_ref().map(|editor| editor.playhead) else {
            return;
        };
        let from = self.stop_playing(Preview::ClipEditor).unwrap_or(playhead);
        self.editor_seek(from + Frame(frames));
    }

    /// Moves the clip editor's playhead to `frame` and shows exactly that frame.
    fn editor_seek(&mut self, frame: Frame) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        editor.playhead = frame.clamp(Frame(0), editor.last_frame());
        self.engine.show(Preview::ClipEditor, editor.playhead);
        self.refresh_editor_transport();
    }

    /// The trim bar's playhead is dragged to `at`, a fraction of the source; `done` when it
    /// was let go.
    pub fn editor_scrub(&mut self, at: f32, done: bool) {
        self.stop_playing(Preview::ClipEditor);
        let rate = self.project.sequence().frame_rate();
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let Some(source) = Source::of(&self.project, &editor.session) else {
            return;
        };
        let at = MediaTime((f64::from(at) * source.duration.0 as f64).round() as i64);
        let length = editor.preview.sequence().end();
        editor.playhead = draft::playhead_at(&editor.session.draft, at, rate, length);
        if done {
            self.engine.show(Preview::ClipEditor, editor.playhead);
        } else {
            self.engine.scrub(Preview::ClipEditor, editor.playhead);
        }
        self.refresh_editor_transport();
    }

    pub fn editor_preview_resized(&self, width: i32, height: i32) {
        if self.editor.is_none() {
            return;
        }
        if let (Ok(width @ 1..), Ok(height @ 1..)) = (u32::try_from(width), u32::try_from(height)) {
            self.engine
                .set_preview_size(Preview::ClipEditor, (width, height));
        }
    }

    /// The engine drew `frame` of the clip editor's preview; while playing, the playhead
    /// follows it.
    pub(crate) fn editor_show_frame(
        &mut self,
        frame: Frame,
        texture: Option<dusk_engine::wgpu::Texture>,
    ) {
        if self.editor.is_none() {
            return;
        }
        let Some(window) = self
            .editor_window
            .as_ref()
            .map(ComponentHandle::clone_strong)
        else {
            return;
        };
        match texture_image(texture) {
            Ok(image) => window.set_preview_image(image),
            Err(error) => return self.editor_fail(&error),
        }
        if self.playing(Preview::ClipEditor).is_some()
            && let Some(editor) = self.editor.as_mut()
        {
            editor.playhead = frame;
            self.refresh_editor_transport();
        }
    }

    /// Playback in the clip editor stopped at `frame`.
    pub(crate) fn editor_stopped(&mut self, frame: Frame) {
        if let Some(editor) = self.editor.as_mut() {
            editor.playhead = frame;
        }
        self.refresh_editor_transport();
    }

    /// A key was pressed in the clip editor; true when it was a shortcut there, or its
    /// question took it.
    pub fn editor_key(&mut self, text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        // Slint moves the keyboard through a dialog's controls with the keys no one takes.
        let dialog = self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.question.is_some())
            || self.export_dialog_in_editor();
        if dialog && crate::keymap::moves_focus(text) {
            return false;
        }
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.question.is_some())
        {
            self.editor_answer_with_key(text);
            return true;
        }
        if self.export_dialog_key(text, true) {
            return true;
        }
        let key = platform::pressed_key();
        match self.keymap.action_for_press(text, key, ctrl, shift, alt) {
            Some(action) => self.editor_act(action),
            None => false,
        }
    }

    /// Does what a shortcut or a button in the clip editor stands for: the transport plays
    /// the draft, the editor's own keys change it, and keys for the whole project work as in
    /// the main window. False for the timeline's keys, which mean nothing here.
    pub fn editor_act(&mut self, action: Action) -> bool {
        let Some(editor) = &self.editor else {
            return false;
        };
        if editor.question.is_some() {
            return true;
        }
        match action {
            Action::PlayPause => self.editor_play_pause(),
            Action::PlayForward => self.editor_play_key(SpeedKey::Faster { forward: true }),
            Action::PlayBackward => self.editor_play_key(SpeedKey::Faster { forward: false }),
            Action::PlaySlowForward => self.editor_play_key(SpeedKey::Slower { forward: true }),
            Action::PlaySlowBackward => self.editor_play_key(SpeedKey::Slower { forward: false }),
            Action::Pause => self.editor_pause(),
            Action::StepBack => self.editor_step(-1),
            Action::StepForward => self.editor_step(1),
            Action::GoToStart => self.editor_seek(Frame(0)),
            Action::GoToEnd => self.editor_seek(Frame(i64::MAX)),
            Action::BackSecond => {
                self.editor_step(-one_second(self.project.sequence().frame_rate()));
            }
            Action::AheadSecond => {
                self.editor_step(one_second(self.project.sequence().frame_rate()));
            }
            Action::MarkIn => self.editor_mark(Edge::Start),
            Action::MarkOut => self.editor_mark(Edge::End),
            Action::TurnLeft => self.editor_turn(false),
            Action::TurnRight => self.editor_turn(true),
            Action::MirrorLeftRight => self.editor_flip(true),
            Action::MirrorTopBottom => self.editor_flip(false),
            Action::ToggleFill => self.editor_toggle_fill(),
            Action::ApplyClip => {
                self.apply_draft();
            }
            Action::CloseClipEditor => self.close_clip_editor(),
            Action::ReloadClip => self.editor_reload(),
            Action::KeepDraft => self.editor_keep_draft(),
            Action::ExportClip => self.export_clip(),
            Action::ShortcutList => {
                if let Some(window) = &self.editor_window {
                    window.invoke_show_shortcut_list();
                }
            }
            Action::Undo
            | Action::Redo
            | Action::NewProject
            | Action::OpenProject
            | Action::Save
            | Action::SaveAs
            | Action::Import
            | Action::Export
            | Action::CompressVideo
            | Action::CancelExport
            | Action::Quit => self.act(action),
            Action::Split
            | Action::Delete
            | Action::RippleDelete
            | Action::DeleteOne
            | Action::Unlink
            | Action::ToggleEnabled
            | Action::Place
            | Action::ToggleMute(_)
            | Action::ToggleLock(_)
            | Action::ZoomIn
            | Action::ZoomOut
            | Action::ZoomFit
            | Action::SequenceSettings
            | Action::Settings
            | Action::About
            | Action::OpenClipEditor
            | Action::PreviousCut
            | Action::NextCut
            | Action::SelectAtPlayhead
            | Action::SelectNone
            | Action::PreviousMedia
            | Action::NextMedia => return false,
        }
        true
    }

    /// Shows the draft and how its clips stand, and the clip editor's transport.
    pub(crate) fn refresh_editor(&self) {
        let (Some(editor), Some(window)) = (&self.editor, &self.editor_window) else {
            return;
        };
        let view = draft::editor_view(&self.project, &editor.session);
        let changed = if view.changed { "*" } else { "" };
        window.set_window_title(format!("{}{changed} — Clip editor", view.name).into());
        let message = if view.video {
            ""
        } else {
            "Sound only: this clip has no picture."
        };
        window.set_preview_message(message.into());
        window.set_props(props(&view));
        self.refresh_editor_transport();
    }

    /// Updates the clip editor's playhead, its readouts and its play button.
    pub(crate) fn refresh_editor_transport(&self) {
        let (Some(editor), Some(window)) = (&self.editor, &self.editor_window) else {
            return;
        };
        let rate = self.project.sequence().frame_rate();
        let playing = self.playing(Preview::ClipEditor);
        window.set_playing(playing.is_some());
        window.set_speed(factor_label(playing).into());
        window.set_position_timecode(timecode(editor.playhead, rate).into());
        let duration =
            Source::of(&self.project, &editor.session).map_or(0, |source| source.duration.0);
        let at = draft::source_time_at(&editor.session.draft, editor.playhead, rate);
        let fraction = if duration > 0 {
            (at.0 as f64 / duration as f64) as f32
        } else {
            0.0
        };
        window.set_playhead(fraction);
    }

    /// Export as file: the export dialog for the draft alone, at the clip's own frame rate
    /// and size (docs/ARCHITECTURE.md, "Pop-out clip editor"); a clip without a picture
    /// exports as sound.
    pub(crate) fn export_clip(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        if self.export.is_some() {
            return self.editor_say("An export is already running.");
        }
        let project = match editor.session.export_project(&self.project) {
            Ok(project) => project,
            Err(rejection) => return self.editor_fail(&sentence(&rejection.to_string())),
        };
        let Some(source) = editor
            .session
            .clips()
            .first()
            .and_then(|clip| self.project.media_ref(clip.media_id))
            .map(|media| media.path.clone())
        else {
            return;
        };
        self.open_export_dialog(ExportTarget::Clip {
            project: Box::new(project),
            source,
        });
    }

    /// Where the clip editor's playhead is, while it is open.
    pub(crate) fn editor_playhead(&self) -> Option<Frame> {
        self.editor.as_ref().map(|editor| editor.playhead)
    }

    /// Shows `message` in the clip editor's status line.
    pub(crate) fn editor_say(&self, message: &str) {
        self.editor_status(message, StatusKind::Info);
    }

    /// Shows `message` in the clip editor's status line as something to notice.
    pub(crate) fn editor_warn(&self, message: &str) {
        self.editor_status(message, StatusKind::Warning);
    }

    /// Shows `message` in the clip editor's status line as an error.
    pub(crate) fn editor_fail(&self, message: &str) {
        self.editor_status(message, StatusKind::Error);
    }

    fn editor_status(&self, message: &str, kind: StatusKind) {
        if let Some(window) = &self.editor_window {
            window.set_status(message.into());
            window.set_status_kind(kind);
        }
    }
}

/// The view as the window's properties.
fn props(view: &EditorView) -> EditorProps {
    let int = |value: i64| i32::try_from(value).unwrap_or(i32::MAX);
    let side = |pixels: u32| i32::try_from(pixels).unwrap_or(i32::MAX);
    EditorProps {
        name: view.name.as_str().into(),
        place: view.place.as_str().into(),
        video: view.video,
        audio: view.audio,
        still: view.still,
        in_point: view.in_point,
        out_point: view.out_point,
        source_in: view.source_in.as_str().into(),
        source_out: view.source_out.as_str().into(),
        length: view.length.as_str().into(),
        speed: view.speed_percent,
        frames: int(view.frames),
        turn: view.turn_degrees,
        flip_h: view.flip_h,
        flip_v: view.flip_v,
        fill: view.fill,
        crop: EditorSides {
            left: side(view.crop.left),
            top: side(view.crop.top),
            right: side(view.crop.right),
            bottom: side(view.crop.bottom),
        },
        picture: view.picture.as_str().into(),
        volume: view.volume,
        fade_in: int(view.fade_in),
        fade_out: int(view.fade_out),
        locked: view.locked,
        changed: view.changed,
        edited: view.edited,
        broken: view.broken,
    }
}

/// Hands the clip editor window's callbacks to the editor.
fn connect(window: &ClipEditorWindow) {
    window.on_preview_resized(|width, height| {
        with_app(|app| app.editor_preview_resized(width, height));
    });
    window.on_play_pause(|| {
        with_app(App::editor_play_pause);
    });
    window.on_scrub(|at, done| {
        with_app(|app| app.editor_scrub(at, done));
    });
    window.on_trim(|start, at, done| {
        with_app(|app| app.editor_trim(start, at, done));
    });
    window.on_set_speed(|percent| {
        with_app(|app| app.editor_set_speed(percent));
    });
    window.on_set_length(|frames| {
        with_app(|app| app.editor_set_length(frames));
    });
    window.on_turn(|clockwise| {
        with_app(|app| app.editor_turn(clockwise));
    });
    window.on_flip(|horizontal| {
        with_app(|app| app.editor_flip(horizontal));
    });
    window.on_set_fill(|fill| {
        with_app(|app| app.editor_set_fill(fill));
    });
    window.on_set_crop(|sides| {
        with_app(|app| app.editor_set_crop(sides));
    });
    window.on_set_volume(|decibels| {
        with_app(|app| app.editor_set_volume(decibels));
    });
    window.on_set_fades(|fade_in, fade_out| {
        with_app(|app| app.editor_set_fades(fade_in, fade_out));
    });
    window.on_prompt_answered(|index| {
        with_app(|app| app.editor_answer(usize::try_from(index).unwrap_or(usize::MAX)));
    });
    window.on_export_changed(|what, value| {
        with_app(|app| app.export_changed(&what, value));
    });
    window.on_export_done(|export| {
        with_app(|app| app.export_done(export));
    });
    window.on_action(|name| {
        if let Some(action) = Action::named(&name) {
            with_app(|app| app.editor_act(action));
        }
    });
    window.on_key(|text, ctrl, shift, alt| {
        with_app(|app| app.editor_key(&text, ctrl, shift, alt)).unwrap_or(false)
    });
    if let Some((list, problem)) =
        with_app(|app| (app.keymap.shortcut_list(), app.keymap_problem.clone()))
    {
        window.set_shortcuts(list);
        window.set_shortcut_problem(problem.into());
    }
    window.window().on_close_requested(|| {
        if with_app(App::may_close_editor).unwrap_or(true) {
            CloseRequestResponse::HideWindow
        } else {
            CloseRequestResponse::KeepWindowShown
        }
    });
    platform::watch_keys(window.window());
}
