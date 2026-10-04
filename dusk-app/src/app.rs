//! The editor's state on the UI thread, and what each user action does to it. Edits go
//! through commands and the undo stack; decoding, playback and export happen in the engine,
//! whose results come back here on the UI thread.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};

use dusk_core::time::STANDARD_RATES;
use dusk_core::{ClipId, Command, Edge, Frame, MediaInfo, Project, TrimClips, import};
use dusk_engine::{Engine, EngineError, EngineEvent, media_info};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::history::History;
use crate::shortcuts::Action;
use crate::timeline::{self, ClipRow};
use crate::{ClipView, MainWindow, TickView};

/// The fastest forward playback the L key reaches (docs/ARCHITECTURE.md, "Playback").
const FASTEST: f64 = 8.0;

thread_local! {
    /// The editor, owned by the UI thread.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Makes `app` the editor that [`with_app`] reaches.
pub fn install(app: App) {
    APP.with(|cell| *cell.borrow_mut() = Some(app));
}

/// Drops the editor, and with it the engine and its threads.
pub fn uninstall() {
    let app = APP.with(|cell| cell.borrow_mut().take());
    drop(app);
}

/// Runs `f` on the editor. Called on the UI thread only; does nothing before the editor is
/// installed, or if it is already in use further up the stack.
pub fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(f))
}

/// What the engine reports, sent on to the UI thread. Frames are passed through a slot that
/// holds only the newest one, so a busy UI thread skips frames instead of queuing them.
pub fn engine_events() -> impl Fn(EngineEvent) + Send + Sync + 'static {
    type Slot = Arc<Mutex<Option<(Frame, Option<dusk_engine::wgpu::Texture>)>>>;
    let newest: Slot = Arc::default();
    move |event| match event {
        EngineEvent::Frame { frame, texture } => {
            let mut slot = newest.lock().unwrap_or_else(PoisonError::into_inner);
            let waiting = slot.replace((frame, texture)).is_some();
            drop(slot);
            if !waiting {
                let newest = Arc::clone(&newest);
                let _ = slint::invoke_from_event_loop(move || {
                    let frame = newest.lock().unwrap_or_else(PoisonError::into_inner).take();
                    if let Some((frame, texture)) = frame {
                        with_app(|app| app.show_frame(frame, texture));
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

/// The editor.
pub struct App {
    window: slint::Weak<MainWindow>,
    engine: Engine,
    project: Option<Arc<Project>>,
    history: History,
    playhead: Frame,
    /// The timeline's width in pixels, for the zoom.
    timeline_width: f32,
    clips: Rc<VecModel<ClipView>>,
    ticks: Rc<VecModel<TickView>>,
}

impl App {
    /// The editor for `window`, playing through `engine`.
    pub fn new(window: &MainWindow, engine: Engine) -> App {
        let clips = Rc::new(VecModel::default());
        let ticks = Rc::new(VecModel::default());
        window.set_clips(ModelRc::from(Rc::clone(&clips)));
        window.set_ticks(ModelRc::from(Rc::clone(&ticks)));
        App {
            window: window.as_weak(),
            engine,
            project: None,
            history: History::default(),
            playhead: Frame(0),
            timeline_width: window.get_timeline_width(),
            clips,
            ticks,
        }
    }

    /// Imports the file at `path` into a new project, probing it off the UI thread.
    pub fn open(&mut self, path: PathBuf) {
        self.say(&format!("Opening {}…", path.display()));
        let spawned = std::thread::Builder::new()
            .name("dusk import".to_owned())
            .spawn(move || {
                let info = media_info(&path);
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| app.opened(path, info));
                });
            });
        if let Err(error) = spawned {
            self.fail(&EngineError::Thread(error).to_string());
        }
    }

    /// Starts the project from the probed file: the sequence takes its rate and size
    /// (docs/ARCHITECTURE.md, "Sequence settings"), and the file lands at the start.
    fn opened(&mut self, path: PathBuf, info: Result<MediaInfo, EngineError>) {
        let info = match info {
            Ok(info) => info,
            Err(error) => return self.fail(&error.to_string()),
        };
        // Audio alone makes a 1920x1080 sequence at 30 fps.
        let (rate, size) = match info.frame_rate {
            Some(rate) if info.has_video => (rate, (info.width, info.height)),
            _ => (STANDARD_RATES[4], (1920, 1080)),
        };
        let mut project = Project::new(rate, size);
        let name = file_name(&path);
        let command = import(&project, path, info, Frame(0));
        self.history = History::default();
        if let Err(rejection) = self.history.apply(command, &mut project) {
            return self.fail(&sentence(&rejection.to_string()));
        }
        if let Some(window) = self.window.upgrade() {
            window.set_preview_message(SharedString::new());
            window.set_has_project(true);
        }
        self.playhead = Frame(0);
        self.set_project(project);
        self.say(&format!("Opened {name}."));
    }

    /// Makes `project` the current one and shows it.
    fn set_project(&mut self, project: Project) {
        let project = Arc::new(project);
        self.engine.set_project(Arc::clone(&project));
        self.project = Some(project);
        self.playhead = self.clamp(self.playhead);
        self.engine.show(self.playhead);
        self.refresh_timeline();
        self.refresh_transport();
    }

    /// Applies `command` as one undoable edit, or says why it was refused.
    fn edit(&mut self, command: Command) {
        let Some(project) = &self.project else {
            return;
        };
        let mut next = Project::clone(project);
        match self.history.apply(command, &mut next) {
            Ok(applied) => {
                let notices: Vec<String> =
                    applied.notices().iter().map(|n| n.to_string()).collect();
                self.set_project(next);
                self.say(&notices.join(" "));
            }
            Err(rejection) => self.fail(&sentence(&rejection.to_string())),
        }
    }

    fn undo(&mut self) {
        let Some(project) = &self.project else {
            return;
        };
        let mut previous = Project::clone(project);
        if self.history.undo(&mut previous) {
            self.set_project(previous);
            self.say("");
        } else {
            self.say("Nothing to undo.");
        }
    }

    fn redo(&mut self) {
        let Some(project) = &self.project else {
            return;
        };
        let mut next = Project::clone(project);
        match self.history.redo(&mut next) {
            Ok(true) => {
                self.set_project(next);
                self.say("");
            }
            Ok(false) => self.say("Nothing to redo."),
            Err(rejection) => self.fail(&sentence(&rejection.to_string())),
        }
    }

    /// A trim handle of `clip` was dragged to `frame`.
    pub fn trim(&mut self, clip: i32, start: bool, frame: i32) {
        let edge = if start { Edge::Start } else { Edge::End };
        let clip = ClipId(u64::try_from(clip).unwrap_or(0));
        self.edit(Command::TrimClips(TrimClips::new(
            clip,
            edge,
            Frame(frame.into()),
        )));
    }

    /// Does what a shortcut stands for.
    pub fn act(&mut self, action: Action) {
        match action {
            Action::PlayPause => self.play_pause(),
            Action::PlayForward => self.play_faster(),
            Action::PlayBackward => self.play(-1.0),
            Action::Pause => self.pause(),
            Action::StepBack => self.step(-1),
            Action::StepForward => self.step(1),
            Action::GoToStart => self.seek(Frame(0)),
            Action::GoToEnd => self.seek(self.last_frame()),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::Export | Action::CancelExport => {}
        }
    }

    pub fn play_pause(&mut self) {
        if self.engine.playing().is_some() {
            self.pause();
        } else {
            self.play(1.0);
        }
    }

    /// L: plays forwards, twice as fast with every press while playing forwards.
    fn play_faster(&mut self) {
        let factor = match self.engine.playing() {
            Some(factor) if factor > 0.0 => (factor * 2.0).min(FASTEST),
            _ => 1.0,
        };
        self.play(factor);
    }

    /// Plays at `factor` from where playback is, or from the playhead.
    fn play(&mut self, factor: f64) {
        if self.project.is_none() {
            return;
        }
        let from = self.engine.pause().unwrap_or(self.playhead);
        // Playing forwards from the last frame starts over.
        let from = if factor > 0.0 && from >= self.last_frame() {
            Frame(0)
        } else {
            from
        };
        self.playhead = from;
        self.engine.play(from, factor);
        self.refresh_transport();
    }

    fn pause(&mut self) {
        if let Some(at) = self.engine.pause() {
            self.playhead = at;
        }
        self.refresh_transport();
    }

    fn step(&mut self, frames: i64) {
        let from = self.engine.pause().unwrap_or(self.playhead);
        self.seek(from + Frame(frames));
    }

    /// Moves the playhead to `frame` and shows exactly that frame.
    fn seek(&mut self, frame: Frame) {
        self.playhead = self.clamp(frame);
        self.engine.show(self.playhead);
        self.refresh_transport();
    }

    /// The playhead is being dragged to `frame`.
    pub fn scrub(&mut self, frame: i32) {
        self.playhead = self.clamp(Frame(frame.into()));
        self.engine.scrub(self.playhead);
        self.refresh_transport();
    }

    /// The playhead was let go at `frame`.
    pub fn scrub_end(&mut self, frame: i32) {
        self.seek(Frame(frame.into()));
    }

    pub fn preview_resized(&self, width: i32, height: i32) {
        if let (Ok(width @ 1..), Ok(height @ 1..)) = (u32::try_from(width), u32::try_from(height)) {
            self.engine.set_preview_size((width, height));
        }
    }

    pub fn timeline_resized(&mut self, width: f32) {
        self.timeline_width = width;
        self.refresh_timeline();
    }

    /// The engine drew `frame`; while playing, the playhead follows it.
    fn show_frame(&mut self, frame: Frame, texture: Option<dusk_engine::wgpu::Texture>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let image = match texture.map(slint::Image::try_from) {
            None => slint::Image::default(),
            Some(Ok(image)) => image,
            Some(Err(error)) => return self.fail(&error.to_string()),
        };
        window.set_preview_image(image);
        if self.engine.playing().is_some() {
            self.playhead = frame;
            self.refresh_transport();
        }
    }

    fn engine_event(&mut self, event: EngineEvent) {
        match event {
            EngineEvent::Stopped { frame } => {
                self.playhead = frame;
                self.refresh_transport();
            }
            EngineEvent::Error(error) => self.fail(&error.to_string()),
            EngineEvent::Frame { frame, texture } => self.show_frame(frame, texture),
        }
    }

    /// Redraws the clips and the ruler.
    fn refresh_timeline(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let Some(project) = &self.project else {
            return;
        };
        let rate = project.sequence().frame_rate();
        let end = project.sequence().end();
        let zoom = timeline::zoom(self.timeline_width, end, rate);
        let clips: Vec<ClipView> = timeline::clip_rows(project)
            .into_iter()
            .map(clip_view)
            .collect();
        self.clips.set_vec(clips);
        let ticks: Vec<TickView> = timeline::ticks(self.timeline_width, zoom, rate)
            .into_iter()
            .map(|tick| TickView {
                frame: frame_int(tick.frame),
                label: tick.label.into(),
            })
            .collect();
        if self.ticks.iter().ne(ticks.iter().cloned()) {
            self.ticks.set_vec(ticks);
        }
        window.set_zoom(zoom);
        window.set_end_timecode(timeline::timecode(end, rate).into());
    }

    /// Updates the playhead and the transport readouts.
    fn refresh_transport(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let Some(project) = &self.project else {
            return;
        };
        let rate = project.sequence().frame_rate();
        window.set_playhead(frame_int(self.playhead));
        window.set_position_timecode(timeline::timecode(self.playhead, rate).into());
        let playing = self.engine.playing();
        window.set_playing(playing.is_some());
        let speed = match playing {
            Some(factor) if factor != 1.0 => format!("{factor}x"),
            _ => String::new(),
        };
        window.set_speed(speed.into());
    }

    /// The last frame of the sequence; 0 when it is empty.
    fn last_frame(&self) -> Frame {
        self.project.as_ref().map_or(Frame(0), |project| {
            (project.sequence().end() - Frame(1)).max(Frame(0))
        })
    }

    fn clamp(&self, frame: Frame) -> Frame {
        frame.clamp(Frame(0), self.last_frame())
    }

    /// Shows `message` in the status line.
    fn say(&self, message: &str) {
        if let Some(window) = self.window.upgrade() {
            window.set_status(message.into());
            window.set_status_is_error(false);
        }
    }

    /// Shows `message` in the status line as an error.
    fn fail(&self, message: &str) {
        if let Some(window) = self.window.upgrade() {
            window.set_status(message.into());
            window.set_status_is_error(true);
        }
    }
}

fn clip_view(row: ClipRow) -> ClipView {
    ClipView {
        id: i32::try_from(row.id).unwrap_or(i32::MAX),
        track: i32::try_from(row.track).unwrap_or(0),
        start: frame_int(row.start),
        length: frame_int(row.length),
        name: row.name.into(),
        link: row
            .link
            .map_or(0, |link| i32::try_from(link).unwrap_or(i32::MAX)),
        video: row.video,
    }
}

/// A frame number for Slint, whose integers are 32-bit: more than two years of frames.
fn frame_int(frame: Frame) -> i32 {
    i32::try_from(frame.0).unwrap_or(if frame.0 < 0 { i32::MIN } else { i32::MAX })
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// `text` with a capital first letter and a full stop, for the status line.
fn sentence(text: &str) -> String {
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
