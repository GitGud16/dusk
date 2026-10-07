//! The project as a file (docs/ARCHITECTURE.md, "Project file and autosave"): new, open,
//! save and save as, the autosave every 30 seconds while there are unsaved changes, crash
//! recovery, and the questions these ask. Reading and writing happen on the file worker.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use dusk_core::file::{FileError, from_json, to_json};
use dusk_core::{MediaId, Project, Rational};
use slint::{ComponentHandle, Model};

use crate::app::{App, file_name, sentence, with_app};
use crate::clip_editor::AfterDraft;
use crate::files::write_atomically;
use crate::platform::{self, Dialog};
use crate::recovery::{self, Leftover, Session};

/// How often unsaved changes are autosaved.
pub const AUTOSAVE_EVERY: Duration = Duration::from_secs(30);

/// The project file the editor works on.
pub struct Document {
    /// Where it was opened from or last saved to; `None` until it is saved.
    pub path: Option<PathBuf>,
    /// The history state that matches the file; never matched when the file is behind.
    pub saved: u64,
    /// The history state last autosaved.
    pub autosaved: u64,
    /// Whether this session's autosave file exists.
    pub autosave_written: bool,
}

/// A history state no history has: the file is behind the project in memory.
const NOT_SAVED: u64 = u64::MAX;

impl Document {
    /// A document for the file at `path`, matching the history state `state`.
    pub fn new(path: Option<PathBuf>, state: u64) -> Document {
        Document {
            path,
            saved: state,
            autosaved: state,
            autosave_written: false,
        }
    }

    /// The name the title bar shows.
    pub fn name(&self) -> String {
        self.path.as_deref().and_then(Path::file_stem).map_or_else(
            || "Untitled".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    }
}

/// What to do once the unsaved changes are dealt with.
pub enum Next {
    Quit,
    NewProject,
    /// Show the dialog to pick a project to open.
    OpenProject,
    /// Open this project file.
    OpenFile(PathBuf),
}

/// A question on screen, and what each answer does.
pub enum Question {
    /// Save, don't save, or cancel, before `Next`.
    UnsavedChanges(Next),
    /// The first video clip differs from the sequence: match it, or keep the settings.
    MatchSequence {
        media: MediaId,
        row: Option<usize>,
        at: dusk_core::Frame,
        rate: Rational,
        size: (u32, u32),
    },
    /// Recover what a Dusk that stopped left, or discard it.
    Recover(Leftover),
}

impl App {
    /// Whether the project differs from its file.
    pub fn is_dirty(&self) -> bool {
        self.history.state() != self.document.saved
    }

    /// Starts this Dusk's autosave session and looks for work an earlier Dusk left, then
    /// opens or imports `files` from the command line.
    pub fn start(&mut self, files: Vec<PathBuf>) {
        let folder = platform::state_dir().map(|dir| dir.join("autosave"));
        self.files.run(move || {
            let started = match &folder {
                Some(folder) => Session::start(folder).map_err(|e| e.to_string()),
                None => Err("there is no folder for it".to_owned()),
            };
            let found = folder
                .as_deref()
                .map(recovery::leftovers)
                .unwrap_or_default();
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| app.started(started, found, files));
            });
        });
    }

    fn started(
        &mut self,
        session: Result<Session, String>,
        mut leftovers: Vec<Leftover>,
        files: Vec<PathBuf>,
    ) {
        match session {
            Ok(mut session) => {
                if let Err(error) = session.record_project(self.document.path.as_deref()) {
                    self.fail(&format!(
                        "Autosave is off: {error}. Save your work yourself until Dusk starts again."
                    ));
                }
                self.session = Some(session);
            }
            Err(error) => self.fail(&format!(
                "Autosave is off: {error}. Save your work yourself until Dusk starts again."
            )),
        }
        self.open_command_line(files);
        // The newest leftover is offered now; any older ones at the next start.
        if !leftovers.is_empty() {
            let leftover = leftovers.remove(0);
            let what = leftover
                .project
                .as_deref()
                .map_or_else(|| "an untitled project".to_owned(), file_name);
            let ago = SystemTime::now()
                .duration_since(leftover.saved)
                .map_or_else(
                    |_| "just now".to_owned(),
                    |ago| format!("{} ago", minutes(ago)),
                );
            self.ask(
                Question::Recover(leftover),
                "Recover unsaved work?",
                &format!(
                    "Dusk closed before {what} was saved. Its autosave is from {ago}. \
                     Recovering replaces the project open now."
                ),
                &["Recover", "Discard", "Not now"],
                Some(1),
            );
        }
    }

    /// Opens the first project file in `files`, or else imports them all and places them on
    /// the timeline one after another, as M1 did with a single clip.
    fn open_command_line(&mut self, files: Vec<PathBuf>) {
        let files = crate::app::absolute(files);
        let project = files.iter().find(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("dusk"))
        });
        match project {
            Some(project) => self.read_project(project.clone()),
            None if !files.is_empty() => self.import_files(files, true),
            None => {}
        }
    }

    /// Shows a question; `answers` lists the buttons, the first the default and the last
    /// what Escape gives; `destructive` is the one that throws work away.
    pub(crate) fn ask(
        &mut self,
        question: Question,
        title: &str,
        message: &str,
        answers: &[&str],
        destructive: Option<usize>,
    ) {
        self.question = Some(question);
        if let Some(window) = self.window() {
            let answers: Vec<slint::SharedString> =
                answers.iter().map(|answer| (*answer).into()).collect();
            window.set_prompt_title(title.into());
            window.set_prompt_message(message.into());
            window.set_prompt_answers(std::rc::Rc::new(slint::VecModel::from(answers)).into());
            window.set_prompt_destructive(destructive.map_or(-1, |index| index as i32));
            window.set_prompt_open(true);
        }
    }

    /// Enter gives a question's first answer, Escape its last.
    pub(crate) fn answer_with_key(&mut self, text: &str) {
        use slint::platform::Key;
        let Some(window) = self.window() else {
            return;
        };
        let last = window.get_prompt_answers().row_count().saturating_sub(1);
        if text == slint::SharedString::from(Key::Return).as_str() {
            self.answer(0);
        } else if text == slint::SharedString::from(Key::Escape).as_str() {
            self.answer(last);
        }
    }

    /// The answer at `index` was given.
    pub fn answer(&mut self, index: usize) {
        let Some(question) = self.question.take() else {
            return;
        };
        if let Some(window) = self.window() {
            window.set_prompt_open(false);
            window.invoke_take_keys();
        }
        match question {
            Question::UnsavedChanges(next) => match index {
                0 => self.save(Some(next)),
                1 => self.proceed(next),
                _ => {}
            },
            Question::MatchSequence {
                media,
                row,
                at,
                rate,
                size,
            } => match index {
                0 => self.place_now(media, row, at, Some((rate, size))),
                1 => self.place_now(media, row, at, None),
                _ => {}
            },
            // "Not now" leaves the autosave for the next start.
            Question::Recover(leftover) => match index {
                0 => self.recover(leftover),
                1 => self.files.run(move || {
                    let _ = leftover.discard();
                }),
                _ => {}
            },
        }
    }

    /// Asks about changes in the clip editor not applied and unsaved changes before `next`,
    /// or goes straight on without any.
    pub(crate) fn after_unsaved_changes(&mut self, next: Next) {
        if self.draft_pending() {
            return self.ask_about_draft(AfterDraft::Then(next));
        }
        if !self.is_dirty() {
            return self.proceed(next);
        }
        let name = self.document.name();
        self.ask(
            Question::UnsavedChanges(next),
            &format!("Save the changes to {name}?"),
            "Your changes are lost if you don't save them.",
            &["Save", "Don't save", "Cancel"],
            Some(1),
        );
    }

    fn proceed(&mut self, next: Next) {
        match next {
            Next::Quit => {
                self.close_editor_now();
                if let Some(window) = self.window() {
                    let _ = window.hide();
                }
            }
            Next::NewProject => {
                self.replace_project(crate::app::empty_project(), None, crate::missing::Ask::No);
                self.say("New project.");
            }
            Next::OpenProject => {
                self.show_dialog(Dialog::OpenProject, |paths| {
                    if let Some(path) = paths.into_iter().next() {
                        with_app(|app| app.read_project(path));
                    }
                });
            }
            Next::OpenFile(path) => self.read_project(path),
        }
    }

    pub fn new_project(&mut self) {
        self.after_unsaved_changes(Next::NewProject);
    }

    pub fn open_project(&mut self) {
        self.after_unsaved_changes(Next::OpenProject);
    }

    /// Opens the project file `path`, asking first about unsaved changes.
    pub fn open_project_file(&mut self, path: PathBuf) {
        self.after_unsaved_changes(Next::OpenFile(path));
    }

    /// Whether Dusk may close now. With unsaved changes it asks first, and closes once they
    /// are saved or let go.
    pub fn may_close(&mut self) -> bool {
        match &self.question {
            // The user is deciding that very thing.
            Some(Question::UnsavedChanges(_)) => return false,
            // Closing leaves an autosave for the next start, and places nothing.
            Some(Question::Recover(_) | Question::MatchSequence { .. }) => {
                self.question = None;
                if let Some(window) = self.window() {
                    window.set_prompt_open(false);
                }
            }
            None => {}
        }
        if self.draft_pending() || self.is_dirty() {
            self.after_unsaved_changes(Next::Quit);
            return false;
        }
        // The clip editor closes with the main window.
        self.close_editor_now();
        true
    }

    /// Closes Dusk, asking first about unsaved changes.
    pub(crate) fn quit(&mut self) {
        if self.may_close() {
            self.proceed(Next::Quit);
        }
    }

    /// Shows a file dialog unless one is open already, and calls `done` with what was
    /// picked.
    pub(crate) fn show_dialog(
        &mut self,
        dialog: Dialog,
        done: impl FnOnce(Vec<PathBuf>) + Send + 'static,
    ) {
        if let Some(window) = self.window() {
            self.show_dialog_over(window.window(), dialog, done);
        }
    }

    /// Shows a file dialog over `owner` unless one is open already, and calls `done` with
    /// what was picked.
    pub(crate) fn show_dialog_over(
        &mut self,
        owner: &slint::Window,
        dialog: Dialog,
        done: impl FnOnce(Vec<PathBuf>) + Send + 'static,
    ) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        platform::show_dialog(owner, dialog, move |paths| {
            with_app(|app| app.dialog_open = false);
            done(paths);
        });
    }

    /// Reads the project file at `path` on the file worker, then shows it.
    pub(crate) fn read_project(&mut self, path: PathBuf) {
        self.say(&format!("Opening {}…", file_name(&path)));
        self.files.run(move || {
            let read = std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|text| from_json(&text, &path).map_err(|error| error.to_string()));
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| app.project_read(path, read));
            });
        });
    }

    /// The project file at `path` was read. Missing media is reported, never dropped
    /// (docs/REQUIREMENTS.md, "Robust"): the check that follows lists it to be found.
    fn project_read(&mut self, path: PathBuf, read: Result<Project, String>) {
        match read {
            Ok(project) => {
                self.replace_project(project, Some(path.clone()), crate::missing::Ask::Opened);
                self.say(&format!("Opened {}.", file_name(&path)));
            }
            Err(error) => self.fail(&format!(
                "Could not open {}: {}",
                file_name(&path),
                sentence(&error)
            )),
        }
    }

    /// Makes `project`, from the file at `path` if any, the one open, as a document of its
    /// own; the check of its media files then does what `ask` says.
    pub(crate) fn replace_project(
        &mut self,
        project: Project,
        path: Option<PathBuf>,
        ask: crate::missing::Ask,
    ) {
        self.history = crate::history::History::default();
        self.document = Document::new(path, self.history.state());
        self.record_project();
        self.selected_clip = None;
        self.selected_media = None;
        self.playhead = dusk_core::Frame(0);
        self.view.fit();
        self.close_editor_now();
        self.engine.pause();
        self.missing.clear();
        self.end_missing_list();
        self.media_check_ask = ask;
        self.set_project(project);
    }

    /// Notes in the session which project file this Dusk edits, for recovery.
    fn record_project(&mut self) {
        let path = self.document.path.clone();
        if let Some(session) = &mut self.session
            && let Err(error) = session.record_project(path.as_deref())
        {
            self.fail(&format!(
                "Autosave may not find this project after a crash: {error}. Save often until \
                 Dusk starts again."
            ));
        }
    }

    /// Saves to the project's file, or asks where when it has none; then `next`.
    pub fn save(&mut self, next: Option<Next>) {
        match self.document.path.clone() {
            Some(path) => self.write_project(path, next),
            None => self.save_as(next),
        }
    }

    /// Asks where to save, then saves there; then `next`.
    pub fn save_as(&mut self, next: Option<Next>) {
        let suggested = format!("{}.dusk", self.document.name());
        self.show_dialog(Dialog::SaveProject { suggested }, move |paths| {
            if let Some(path) = paths.into_iter().next() {
                with_app(|app| app.write_project(path, next));
            }
        });
    }

    fn write_project(&mut self, path: PathBuf, next: Option<Next>) {
        let text = to_json(&self.project, &path);
        let state = self.history.state();
        self.say(&format!("Saving {}…", file_name(&path)));
        self.files.run(move || {
            let written = write_atomically(&path, text.as_bytes()).map_err(|e| e.to_string());
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| app.project_written(path, state, written, next));
            });
        });
    }

    fn project_written(
        &mut self,
        path: PathBuf,
        state: u64,
        written: Result<(), String>,
        next: Option<Next>,
    ) {
        if let Err(error) = written {
            return self.fail(&format!(
                "Could not save {}: {}. Try Save as to put it somewhere else.",
                file_name(&path),
                error
            ));
        }
        let moved = self.document.path.as_deref() != Some(path.as_path());
        self.document.path = Some(path.clone());
        self.document.saved = state;
        if moved {
            self.record_project();
        }
        self.refresh_title();
        self.say(&format!("Saved {}.", file_name(&path)));
        if let Some(next) = next {
            self.proceed(next);
        }
    }

    /// Every [`AUTOSAVE_EVERY`]: writes the project to the session's autosave while it has
    /// unsaved changes, and removes the autosave once it has none.
    pub fn autosave(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let autosave = session.autosave();
        let state = self.history.state();
        if self.is_dirty() {
            if state == self.document.autosaved && self.document.autosave_written {
                return;
            }
            // Relative paths in the autosave are relative to the project's folder, as in the
            // project file, so recovery reads them back the same way.
            let base = self
                .document
                .path
                .clone()
                .unwrap_or_else(|| autosave.clone());
            let text = to_json(&self.project, &base);
            self.document.autosaved = state;
            self.document.autosave_written = true;
            self.files.run(move || {
                if let Err(error) = write_atomically(&autosave, text.as_bytes()) {
                    let message = format!("Autosave failed: {error}.");
                    let _ = slint::invoke_from_event_loop(move || {
                        with_app(|app| app.fail(&message));
                    });
                }
            });
        } else if self.document.autosave_written {
            self.document.autosave_written = false;
            self.files.run(move || {
                let _ = std::fs::remove_file(&autosave);
            });
        }
    }

    /// Reads a leftover autosave and makes it the project, unsaved.
    fn recover(&mut self, leftover: Leftover) {
        let base = leftover
            .project
            .clone()
            .unwrap_or_else(|| leftover.autosave.clone());
        self.files.run(move || {
            let read = std::fs::read_to_string(&leftover.autosave)
                .map_err(|error| Unrecovered {
                    what: error.to_string(),
                    newer: false,
                })
                .and_then(|text| from_json(&text, &base).map_err(Unrecovered::from));
            let project = leftover.project.clone();
            // The project now lives in this session; the old one's files go.
            if read.is_ok() {
                let _ = leftover.discard();
            }
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| app.recovered(project, read));
            });
        });
    }

    fn recovered(&mut self, path: Option<PathBuf>, read: Result<Project, Unrecovered>) {
        match read {
            Ok(project) => {
                self.replace_project(project, path, crate::missing::Ask::Recovered);
                // Unsaved until saved: the recovered work exists only in memory now.
                self.document.saved = NOT_SAVED;
                self.refresh_title();
                self.say("Recovered. Save to keep it.");
            }
            Err(error) => self.fail(&recovery_failed(&error, path.is_some())),
        }
    }

    /// Ends the session as Dusk closes normally: its autosave goes.
    pub fn shut_down(&mut self) {
        if let Some(session) = self.session.take() {
            self.files.run(move || {
                let _ = session.close();
            });
        }
    }
}

/// Why an autosave could not be recovered: what is wrong with it, without the advice for a
/// file the user picked, since they can neither change an autosave nor find another copy of
/// it; and whether a newer Dusk wrote it, which can still recover it.
#[derive(Debug)]
struct Unrecovered {
    what: String,
    newer: bool,
}

impl From<FileError> for Unrecovered {
    fn from(error: FileError) -> Unrecovered {
        match error {
            // Its advice, a newer Dusk, holds for an autosave too.
            FileError::Version(_) => Unrecovered {
                what: error.to_string(),
                newer: true,
            },
            error => Unrecovered {
                what: error.what(),
                newer: false,
            },
        }
    }
}

/// What to say when an autosave could not be recovered: keep one a newer Dusk wrote, which
/// stays to be offered again, or else open the project's file, when it was `saved` as one.
fn recovery_failed(unrecovered: &Unrecovered, saved: bool) -> String {
    let instead = if unrecovered.newer {
        "Choose Not now when Dusk asks again, to keep it for a newer Dusk."
    } else if saved {
        "Open the project's last saved file instead."
    } else {
        "The project was never saved, so no other file holds its work."
    };
    format!(
        "Could not recover the autosave: {} {instead}",
        sentence(&unrecovered.what)
    )
}

/// `duration` in whole minutes, for messages.
fn minutes(duration: Duration) -> String {
    match duration.as_secs() / 60 {
        0 => "less than a minute".to_owned(),
        1 => "a minute".to_owned(),
        minutes => format!("{minutes} minutes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_recovery_points_a_saved_project_at_its_file() {
        let broken = Unrecovered::from(FileError::Invalid(dusk_core::Rejection::SourceRange(
            dusk_core::ClipId(4),
        )));
        assert_eq!(
            recovery_failed(&broken, true),
            "Could not recover the autosave: The project file breaks a timeline rule (the clip \
             reaches outside its source file). Open the project's last saved file instead."
        );
    }

    #[test]
    fn an_autosave_from_a_newer_dusk_is_kept_for_it() {
        // The autosave stays, and Dusk offers it again at its next start.
        let newer = Unrecovered {
            what: "the project file is in format version 2, which this Dusk cannot open; a newer \
                   Dusk may"
                .to_owned(),
            newer: true,
        };
        assert_eq!(
            recovery_failed(&newer, false),
            "Could not recover the autosave: The project file is in format version 2, which \
             this Dusk cannot open; a newer Dusk may. Choose Not now when Dusk asks again, to \
             keep it for a newer Dusk."
        );
    }

    #[test]
    fn a_failed_recovery_of_an_untitled_project_has_no_file_to_point_at() {
        assert_eq!(
            recovery_failed(
                &Unrecovered {
                    what: "access is denied. (os error 5)".to_owned(),
                    newer: false,
                },
                false
            ),
            "Could not recover the autosave: Access is denied. (os error 5). The project was \
             never saved, so no other file holds its work."
        );
    }
}
