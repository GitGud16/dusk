//! The editor's state on the UI thread, and what each user action does to it. Edits go
//! through commands and the undo stack; decoding, playback and export happen in the engine
//! and file work on a worker, whose results come back here on the UI thread. Project files
//! and autosave are in `document`, timeline edits in `editing`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use dusk_core::time::STANDARD_RATES;
use dusk_core::{ClipId, Command, Frame, MediaId, Project};
use dusk_engine::{Engine, EngineEvent, ExportEvent, ExportJob, Preview};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::clip_editor::ClipEditor;
use crate::document::{Document, Question};
use crate::files::{Worker, export_path};
use crate::history::History;
use crate::recovery::Session;
use crate::shortcuts::Action;
use crate::speed::{SpeedKey, next_factor};
use crate::stats::Stats;
use crate::thumbnails::{THUMBNAIL_CAP, Thumbnails};
use crate::timeline::{self, View};
use crate::{ClipEditorWindow, ClipProps, ClipView, MainWindow, MediaView, TickView, TrackView};

thread_local! {
    /// The editor, owned by the UI thread.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Makes `app` the editor that [`with_app`] reaches.
pub fn install(app: App) {
    APP.with(|cell| *cell.borrow_mut() = Some(app));
}

/// Drops the editor, and with it the engine, its threads and the file worker.
pub fn uninstall() {
    let app = APP.with(|cell| cell.borrow_mut().take());
    drop(app);
}

/// Runs `f` on the editor. Called on the UI thread only; does nothing before the editor is
/// installed, or if it is already in use further up the stack.
pub fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(f))
}

/// What the engine reports, sent on to the UI thread. Frames are passed through a slot per
/// preview that holds only the newest one, so a busy UI thread skips frames instead of
/// queuing them.
pub fn engine_events() -> impl Fn(EngineEvent) + Send + Sync + 'static {
    type Newest = Option<(Frame, Option<dusk_engine::wgpu::Texture>)>;
    let slots: Arc<Mutex<[Newest; 2]>> = Arc::default();
    move |event| match event {
        EngineEvent::Frame {
            preview,
            frame,
            texture,
        } => {
            let mut newest = slots.lock().unwrap_or_else(PoisonError::into_inner);
            let waiting = newest.iter().any(Option::is_some);
            newest[preview.index()] = Some((frame, texture));
            drop(newest);
            if !waiting {
                let slots = Arc::clone(&slots);
                let _ = slint::invoke_from_event_loop(move || {
                    let newest =
                        std::mem::take(&mut *slots.lock().unwrap_or_else(PoisonError::into_inner));
                    for (preview, newest) in Preview::ALL.into_iter().zip(newest) {
                        if let Some((frame, texture)) = newest {
                            with_app(|app| app.show_frame(preview, frame, texture));
                        }
                    }
                });
            }
        }
        other => {
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| app.engine_event(other));
            });
        }
    }
}

/// The models the window's lists are drawn from.
struct Models {
    tracks: Rc<VecModel<TrackView>>,
    clips: Rc<VecModel<ClipView>>,
    ticks: Rc<VecModel<TickView>>,
    media: Rc<VecModel<MediaView>>,
}

/// The editor.
pub struct App {
    pub(crate) window: slint::Weak<MainWindow>,
    pub(crate) engine: Engine,
    pub(crate) project: Arc<Project>,
    pub(crate) history: History,
    pub(crate) document: Document,
    /// This Dusk's autosave session; `None` while it starts, or if it could not.
    pub(crate) session: Option<Session>,
    pub(crate) files: Worker,
    pub(crate) playhead: Frame,
    pub(crate) selected_clip: Option<ClipId>,
    pub(crate) selected_media: Option<MediaId>,
    /// The export that is running, if one is.
    export: Option<ExportJob>,
    pub(crate) view: View,
    /// The question on screen, if one is.
    pub(crate) question: Option<Question>,
    /// A file dialog is open; others wait until it closes.
    pub(crate) dialog_open: bool,
    pub(crate) sequence_settings_open: bool,
    models: Models,
    /// The media bin's thumbnails.
    thumbnails: Thumbnails<slint::Image>,
    /// Preview statistics, when `DUSK_STATS` is set.
    stats: Option<Stats>,
    /// The clip editor's work, while it is open.
    pub(crate) editor: Option<ClipEditor>,
    /// The clip editor's window, made when it first opens and kept for the next time.
    pub(crate) editor_window: Option<ClipEditorWindow>,
}

impl App {
    /// The editor for `window`, playing through `engine`, with an empty project and the file
    /// worker `files`.
    pub fn new(window: &MainWindow, engine: Engine, files: Worker) -> App {
        let models = Models {
            tracks: Rc::new(VecModel::default()),
            clips: Rc::new(VecModel::default()),
            ticks: Rc::new(VecModel::default()),
            media: Rc::new(VecModel::default()),
        };
        window.set_tracks(ModelRc::from(Rc::clone(&models.tracks)));
        window.set_clips(ModelRc::from(Rc::clone(&models.clips)));
        window.set_ticks(ModelRc::from(Rc::clone(&models.ticks)));
        window.set_media(ModelRc::from(Rc::clone(&models.media)));
        let history = History::default();
        let mut app = App {
            window: window.as_weak(),
            engine,
            project: Arc::new(empty_project()),
            document: Document::new(None, history.state()),
            history,
            session: None,
            files,
            playhead: Frame(0),
            selected_clip: None,
            selected_media: None,
            export: None,
            view: View::new(window.get_timeline_width()),
            question: None,
            dialog_open: false,
            sequence_settings_open: false,
            models,
            thumbnails: Thumbnails::new(THUMBNAIL_CAP),
            stats: Stats::start(window),
            editor: None,
            editor_window: None,
        };
        app.set_project(Project::clone(&app.project));
        app
    }

    pub(crate) fn window(&self) -> Option<MainWindow> {
        self.window.upgrade()
    }

    /// Makes `project` the current one and shows it.
    pub(crate) fn set_project(&mut self, project: Project) {
        let project = Arc::new(project);
        self.engine.set_project(Preview::Main, Arc::clone(&project));
        self.project = project;
        if self
            .selected_clip
            .is_some_and(|clip| self.project.find_clip(clip).is_none())
        {
            self.selected_clip = None;
        }
        if self
            .selected_media
            .is_some_and(|media| self.project.media_ref(media).is_none())
        {
            self.selected_media = None;
        }
        self.playhead = self.clamp(self.playhead);
        self.engine.show(Preview::Main, self.playhead);
        self.refresh_all();
        self.editor_follow_project();
    }

    /// Applies `command` as one undoable edit and says what it did beyond what was asked, or
    /// why it was refused. True when it was applied.
    pub(crate) fn edit(&mut self, command: Command) -> bool {
        match self.try_edit(command) {
            Ok(notices) => {
                self.say(&notices);
                true
            }
            Err(reason) => {
                self.fail(&reason);
                false
            }
        }
    }

    /// Applies `command` as one undoable edit: what it did beyond what was asked, or why it
    /// was refused.
    pub(crate) fn try_edit(&mut self, command: Command) -> Result<String, String> {
        let mut next = Project::clone(&self.project);
        match self.history.apply(command, &mut next) {
            Ok(applied) => {
                let notices: Vec<String> =
                    applied.notices().iter().map(|n| n.to_string()).collect();
                self.set_project(next);
                Ok(notices.join(" "))
            }
            Err(rejection) => Err(sentence(&rejection.to_string())),
        }
    }

    fn undo(&mut self) {
        let mut previous = Project::clone(&self.project);
        if self.history.undo(&mut previous) {
            self.set_project(previous);
            self.say("");
        } else {
            self.say("Nothing to undo.");
        }
    }

    fn redo(&mut self) {
        let mut next = Project::clone(&self.project);
        match self.history.redo(&mut next) {
            Ok(true) => {
                self.set_project(next);
                self.say("");
            }
            Ok(false) => self.say("Nothing to redo."),
            Err(rejection) => self.fail(&sentence(&rejection.to_string())),
        }
    }

    /// A key was pressed; true when it was a shortcut, or a dialog took it.
    pub fn key(&mut self, text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        if self.question.is_some() {
            self.answer_with_key(text);
            return true;
        }
        if self.sequence_settings_open {
            if text == SharedString::from(slint::platform::Key::Escape).as_str() {
                self.close_sequence_settings();
            }
            return true;
        }
        let key = crate::platform::pressed_key();
        let Some(action) = crate::shortcuts::action_for_key(text, key, ctrl, shift, alt) else {
            return false;
        };
        self.act(action);
        true
    }

    /// Does what a shortcut, a menu item or a button stands for.
    pub fn act(&mut self, action: Action) {
        if self.question.is_some() {
            return;
        }
        match action {
            Action::PlayPause => self.play_pause(),
            Action::PlayForward => self.play_key(SpeedKey::Faster { forward: true }),
            Action::PlayBackward => self.play_key(SpeedKey::Faster { forward: false }),
            Action::PlaySlowForward => self.play_key(SpeedKey::Slower { forward: true }),
            Action::PlaySlowBackward => self.play_key(SpeedKey::Slower { forward: false }),
            Action::Pause => self.pause(),
            Action::StepBack => self.step(-1),
            Action::StepForward => self.step(1),
            Action::GoToStart => self.seek(Frame(0)),
            Action::GoToEnd => self.seek(self.last_frame()),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::Split => self.split(),
            Action::Delete => self.delete(false),
            Action::RippleDelete => self.delete(true),
            Action::DeleteOne => self.delete_one(),
            Action::Unlink => self.unlink(),
            Action::ToggleEnabled => self.toggle_enabled(),
            Action::Place => self.place_selected_at_playhead(),
            Action::ToggleMute(index) => self.toggle_track(index, false),
            Action::ToggleLock(index) => self.toggle_track(index, true),
            Action::ZoomIn => self.zoom_by(1.5, self.playhead),
            Action::ZoomOut => self.zoom_by(1.0 / 1.5, self.playhead),
            Action::ZoomFit => {
                self.view.fit();
                self.refresh_timeline();
            }
            Action::NewProject => self.new_project(),
            Action::OpenProject => self.open_project(),
            Action::Save => self.save(None),
            Action::SaveAs => self.save_as(None),
            Action::Import => self.import_dialog(),
            Action::SequenceSettings => self.open_sequence_settings(),
            Action::Export => self.export(),
            Action::CancelExport => self.cancel_export(),
            Action::Quit => self.quit(),
            Action::ShortcutList => {
                if let Some(window) = self.window() {
                    window.invoke_show_shortcut_list();
                }
            }
            Action::ToggleFill => self.toggle_fill(),
            Action::OpenClipEditor => self.open_selected_clip(),
            // The clip editor's own keys mean nothing in the main window.
            Action::MarkIn
            | Action::MarkOut
            | Action::TurnLeft
            | Action::TurnRight
            | Action::MirrorLeftRight
            | Action::MirrorTopBottom
            | Action::ApplyClip
            | Action::CloseClipEditor => {}
        }
    }

    /// Exports the timeline to an MP4 beside the project file, or beside the first media
    /// file while the project is unsaved (the export dialog arrives in M4).
    pub fn export(&mut self) {
        if self.export.is_some() {
            return self.say("An export is already running.");
        }
        if self.project.sequence().end() == Frame(0) {
            return self.fail("Place some media on the timeline before exporting.");
        }
        let base = match &self.document.path {
            Some(path) => path.clone(),
            None => match self.project.media().first() {
                Some(media) => media.path.clone(),
                None => return self.fail("Import some media before exporting."),
            },
        };
        let path = export_path(&base);
        match self.engine.export(Arc::clone(&self.project), path.clone()) {
            Ok(job) => {
                self.export = Some(job);
                if let Some(window) = self.window() {
                    window.set_exporting(true);
                    window.set_export_progress(0.0);
                }
                self.refresh_transport();
                self.say(&format!("Exporting to {}…", path.display()));
            }
            Err(error) => self.fail(&error.to_string()),
        }
    }

    pub fn cancel_export(&mut self) {
        if let Some(job) = &self.export {
            job.cancel();
            self.say("Cancelling the export…");
        }
    }

    fn export_event(&mut self, event: ExportEvent) {
        let Some(window) = self.window() else {
            return;
        };
        if let ExportEvent::Progress { done, total } = event {
            window.set_export_progress(done as f32 / total.max(1) as f32);
            return;
        }
        self.export = None;
        window.set_exporting(false);
        match event {
            ExportEvent::Finished { path, encoder } => {
                self.say(&format!("Exported to {} ({encoder}).", path.display()));
            }
            ExportEvent::Cancelled => self.say("Export cancelled; nothing was written."),
            ExportEvent::Failed(error) => self.fail(&error.to_string()),
            ExportEvent::Progress { .. } => {}
        }
        // The preview showed cached frames only while exporting.
        self.engine.show(Preview::Main, self.playhead);
    }

    pub fn play_pause(&mut self) {
        if self.playing(Preview::Main).is_some() {
            self.pause();
        } else {
            self.play(1.0);
        }
    }

    /// J, L and Shift with them: plays at the speed the key goes to from the current one.
    fn play_key(&mut self, key: SpeedKey) {
        self.play(next_factor(self.playing(Preview::Main), key));
    }

    /// Plays at `factor` from where playback is, or from the playhead.
    fn play(&mut self, factor: f64) {
        if self.project.sequence().end() == Frame(0) {
            return;
        }
        let from = self.stop_playing(Preview::Main).unwrap_or(self.playhead);
        // Playing forwards from the last frame starts over; backwards from the first frame,
        // from the end.
        let from = if factor > 0.0 && from >= self.last_frame() {
            Frame(0)
        } else if factor < 0.0 && from <= Frame(0) {
            self.last_frame()
        } else {
            from
        };
        self.playhead = from;
        self.engine.play(Preview::Main, from, factor);
        self.refresh_transport();
    }

    pub(crate) fn pause(&mut self) {
        self.stop_playing(Preview::Main);
        self.refresh_transport();
    }

    /// The playback factor, when `preview` is the one playing.
    pub(crate) fn playing(&self, preview: Preview) -> Option<f64> {
        let (playing, factor) = self.engine.playing()?;
        (playing == preview).then_some(factor)
    }

    /// Stops playback, in whichever window it was; that window's playhead stays where it
    /// stopped. Returns that frame when `preview` was the one playing.
    pub(crate) fn stop_playing(&mut self, preview: Preview) -> Option<Frame> {
        let (played, at) = self.engine.pause()?;
        self.stopped(played, at);
        (played == preview).then_some(at)
    }

    /// Playback in `preview` stopped at `frame`.
    fn stopped(&mut self, preview: Preview, frame: Frame) {
        match preview {
            Preview::Main => self.playhead = frame,
            Preview::ClipEditor => self.editor_stopped(frame),
        }
    }

    fn step(&mut self, frames: i64) {
        let from = self.stop_playing(Preview::Main).unwrap_or(self.playhead);
        self.seek(from + Frame(frames));
    }

    /// Moves the playhead to `frame` and shows exactly that frame.
    pub(crate) fn seek(&mut self, frame: Frame) {
        self.playhead = self.clamp(frame);
        self.engine.show(Preview::Main, self.playhead);
        self.refresh_transport();
    }

    /// The playhead is being dragged to `frame`.
    pub fn scrub(&mut self, frame: i32) {
        self.playhead = self.clamp(Frame(frame.into()));
        self.engine.scrub(Preview::Main, self.playhead);
        self.refresh_transport();
    }

    /// The playhead was let go at `frame`.
    pub fn scrub_end(&mut self, frame: i32) {
        self.seek(Frame(frame.into()));
    }

    pub fn preview_resized(&self, width: i32, height: i32) {
        if let (Ok(width @ 1..), Ok(height @ 1..)) = (u32::try_from(width), u32::try_from(height)) {
            self.engine.set_preview_size(Preview::Main, (width, height));
        }
    }

    pub fn timeline_resized(&mut self, width: f32) {
        self.view.width = width;
        self.refresh_timeline();
    }

    /// The mouse wheel scrolled the timeline by `frames`.
    pub fn scroll_by(&mut self, frames: i32) {
        let sequence = self.project.sequence();
        let (end, rate) = (sequence.end(), sequence.frame_rate());
        self.view.scroll_by(frames.into(), end, rate);
        self.refresh_timeline();
    }

    /// Zooms the timeline by `factor` around `frame`.
    pub fn zoom_by(&mut self, factor: f32, frame: Frame) {
        let sequence = self.project.sequence();
        let (end, rate) = (sequence.end(), sequence.frame_rate());
        self.view.zoom_by(factor, frame, end, rate);
        self.refresh_timeline();
    }

    /// The engine drew `frame` in `preview`; while playing, the playhead follows it.
    fn show_frame(
        &mut self,
        preview: Preview,
        frame: Frame,
        texture: Option<dusk_engine::wgpu::Texture>,
    ) {
        let started = Instant::now();
        if preview == Preview::ClipEditor {
            return self.editor_show_frame(frame, texture);
        }
        let Some(window) = self.window() else {
            return;
        };
        match texture_image(texture) {
            Ok(image) => window.set_preview_image(image),
            Err(error) => return self.fail(&error),
        }
        if self.playing(Preview::Main).is_some() {
            self.playhead = frame;
            self.refresh_transport();
        }
        if let Some(stats) = &self.stats {
            stats.frame(started);
        }
    }

    fn engine_event(&mut self, event: EngineEvent) {
        match event {
            EngineEvent::Stopped { preview, frame } => {
                self.stopped(preview, frame);
                self.refresh_transport();
            }
            EngineEvent::Error(error) => self.fail(&error.to_string()),
            EngineEvent::Frame {
                preview,
                frame,
                texture,
            } => self.show_frame(preview, frame, texture),
            EngineEvent::Export(event) => self.export_event(event),
            EngineEvent::Thumbnail { media, thumbnail } => {
                let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &thumbnail.rgba,
                    thumbnail.width,
                    thumbnail.height,
                );
                let bytes = thumbnail.rgba.len();
                self.thumbnails
                    .insert(media, slint::Image::from_rgba8(pixels), bytes);
                self.refresh_bin();
            }
        }
    }

    /// Redraws everything that shows the project.
    pub(crate) fn refresh_all(&mut self) {
        self.refresh_timeline();
        self.refresh_bin();
        self.refresh_properties();
        self.refresh_transport();
        self.refresh_title();
    }

    /// Redraws the tracks, the clips and the ruler.
    pub(crate) fn refresh_timeline(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        let project = Arc::clone(&self.project);
        let sequence = project.sequence();
        let rate = sequence.frame_rate();
        let end = sequence.end();
        let tracks: Vec<TrackView> = timeline::track_rows(&project)
            .into_iter()
            .map(|row| TrackView {
                id: id_int(row.id.0),
                name: row.name.into(),
                video: row.video,
                locked: row.locked,
                muted: row.muted,
            })
            .collect();
        let video_rows = tracks.iter().filter(|track| track.video).count();
        // A model is replaced only when it changed: replacing it rebuilds its items, and an
        // item under a press loses the drag it was following.
        replace_if_changed(&self.models.tracks, tracks);
        let clips: Vec<ClipView> = timeline::clip_rows(&project)
            .into_iter()
            .map(|row| ClipView {
                id: id_int(row.id.0),
                row: i32::try_from(row.row).unwrap_or(0),
                start: frame_int(row.start),
                length: frame_int(row.length),
                name: row.name.into(),
                link: row.link.map_or(0, id_int),
                video: row.video,
                enabled: row.enabled,
            })
            .collect();
        replace_if_changed(&self.models.clips, clips);
        let zoom = self.view.pixels_per_frame(end, rate);
        let first = self.view.first_frame();
        let ticks: Vec<TickView> = timeline::ticks(self.view.width, zoom, first, rate)
            .into_iter()
            .map(|tick| TickView {
                frame: frame_int(tick.frame),
                label: tick.label.into(),
            })
            .collect();
        replace_if_changed(&self.models.ticks, ticks);
        window.set_video_rows(i32::try_from(video_rows).unwrap_or(0));
        window.set_zoom(zoom);
        window.set_scroll(frame_int(first));
        self.refresh_selection();
        window.set_end_timecode(timeline::timecode(end, rate).into());
        window.set_has_clips(end > Frame(0));
        let message = if project.media().is_empty() {
            "Import video, audio or photos with Import, or drop them on the window. Then drag \
             them onto the timeline."
        } else if end == Frame(0) {
            "Drag media from the bin onto the timeline, or double-click it to place it at the \
             playhead."
        } else {
            ""
        };
        window.set_preview_message(message.into());
    }

    /// Marks the selected clip and its link group on the timeline.
    pub(crate) fn refresh_selection(&self) {
        let Some(window) = self.window() else {
            return;
        };
        window.set_selected_clip(self.selected_clip.map_or(-1, |clip| id_int(clip.0)));
        let link = self
            .selected_clip
            .and_then(|clip| self.project.find_clip(clip))
            .and_then(|(_, clip)| clip.link);
        window.set_selected_link(link.map_or(0, |link| id_int(link.0)));
    }

    /// Relists the media bin, and asks the engine for the thumbnails it does not have yet.
    pub(crate) fn refresh_bin(&mut self) {
        for media in self.thumbnails.wanted(&self.project) {
            if let Some(media_ref) = self.project.media_ref(media) {
                let (path, info) = (media_ref.path.clone(), media_ref.info.clone());
                self.engine.make_thumbnail(media, path, info);
            }
        }
        let Some(window) = self.window() else {
            return;
        };
        let media: Vec<MediaView> = timeline::media_rows(&self.project)
            .into_iter()
            .map(|row| MediaView {
                id: id_int(row.id.0),
                name: row.name.into(),
                detail: row.detail.into(),
                length: frame_int(row.length),
                video: row.video,
                audio: row.audio,
                thumbnail: self.thumbnails.get(row.id).unwrap_or_default(),
            })
            .collect();
        replace_if_changed(&self.models.media, media);
        window.set_selected_media(self.selected_media.map_or(-1, |media| id_int(media.0)));
    }

    /// Shows the selected clip's properties.
    pub(crate) fn refresh_properties(&self) {
        let Some(window) = self.window() else {
            return;
        };
        let details = timeline::clip_details(&self.project, self.selected_clip);
        window.set_clip_props(ClipProps {
            shown: details.shown,
            name: details.name.into(),
            place: details.place.into(),
            video: details.video,
            still: details.still,
            enabled: details.enabled,
            linked: details.linked,
            locked: details.locked,
            start: details.start.into(),
            length: details.length.into(),
            frames: i32::try_from(details.frames).unwrap_or(i32::MAX),
            fill: details.fill,
            volume: details.volume,
            fade_in: i32::try_from(details.fade_in).unwrap_or(i32::MAX),
            fade_out: i32::try_from(details.fade_out).unwrap_or(i32::MAX),
        });
    }

    /// Updates the playhead and the transport readouts.
    pub(crate) fn refresh_transport(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        let rate = self.project.sequence().frame_rate();
        let playing = self.playing(Preview::Main);
        if playing.is_some() {
            let before = self.view;
            self.view.follow(self.playhead);
            if self.view != before {
                self.refresh_timeline();
            }
        }
        window.set_playhead(frame_int(self.playhead));
        window.set_position_timecode(timeline::timecode(self.playhead, rate).into());
        window.set_playing(playing.is_some());
        window.set_speed(factor_label(playing).into());
    }

    /// The window title: the project's name, marked while it has unsaved changes.
    pub(crate) fn refresh_title(&self) {
        if let Some(window) = self.window() {
            let changed = if self.is_dirty() { "*" } else { "" };
            let title = format!("{}{changed} — Dusk", self.document.name());
            window.set_window_title(title.into());
        }
    }

    /// The last frame of the sequence; 0 when it is empty.
    pub(crate) fn last_frame(&self) -> Frame {
        (self.project.sequence().end() - Frame(1)).max(Frame(0))
    }

    fn clamp(&self, frame: Frame) -> Frame {
        frame.clamp(Frame(0), self.last_frame())
    }

    /// Shows `message` in the status line.
    pub(crate) fn say(&self, message: &str) {
        if let Some(window) = self.window() {
            window.set_status(message.into());
            window.set_status_is_error(false);
        }
    }

    /// Shows `message` in the status line as an error.
    pub(crate) fn fail(&self, message: &str) {
        if let Some(window) = self.window() {
            window.set_status(message.into());
            window.set_status_is_error(true);
        }
    }

    /// The bin's item for media `id`, for the item being dragged; empty if there is none.
    pub fn media_view(&self, id: i32) -> MediaView {
        self.models
            .media
            .iter()
            .find(|media| media.id == id)
            .unwrap_or_default()
    }
}

/// A preview frame as an image for Slint; none is a black frame.
pub(crate) fn texture_image(
    texture: Option<dusk_engine::wgpu::Texture>,
) -> Result<slint::Image, String> {
    match texture.map(slint::Image::try_from) {
        None => Ok(slint::Image::default()),
        Some(Ok(image)) => Ok(image),
        Some(Err(error)) => Err(error.to_string()),
    }
}

/// The playback factor beside the play button, such as "2x"; nothing at normal speed or
/// while stopped.
pub(crate) fn factor_label(playing: Option<f64>) -> String {
    match playing {
        Some(factor) if factor != 1.0 => format!("{factor}x"),
        _ => String::new(),
    }
}

/// Gives `model` the items `items`, unless it has them already.
fn replace_if_changed<T: Clone + PartialEq + 'static>(model: &VecModel<T>, items: Vec<T>) {
    if model.iter().ne(items.iter().cloned()) {
        model.set_vec(items);
    }
}

/// A new project's sequence: 1920x1080 at 30 fps until the first video clip says otherwise
/// (docs/ARCHITECTURE.md, "Sequence settings").
pub(crate) fn empty_project() -> Project {
    Project::new(STANDARD_RATES[4], (1920, 1080))
}

/// An id for Slint, whose integers are 32-bit.
pub(crate) fn id_int(id: u64) -> i32 {
    i32::try_from(id).unwrap_or(i32::MAX)
}

/// A frame number for Slint, whose integers are 32-bit: more than two years of frames.
pub(crate) fn frame_int(frame: Frame) -> i32 {
    i32::try_from(frame.0).unwrap_or(if frame.0 < 0 { i32::MIN } else { i32::MAX })
}

/// The file name of `path`, for messages.
pub(crate) fn file_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// `text` with a capital first letter and a full stop, for the status line.
pub(crate) fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    let mut sentence: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return String::new(),
    };
    if !sentence.ends_with('.') {
        sentence.push('.');
    }
    sentence
}

/// Paths made absolute against the current folder, as the project file needs them.
pub(crate) fn absolute(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .map(|path| std::path::absolute(&path).unwrap_or(path))
        .collect()
}
