//! Media files a project cannot find (docs/ARCHITECTURE.md, "Release (0.1)"): a dialog lists
//! them, and finding one relinks it, with the other missing files that lie beside it, as one
//! undoable edit. What is found and how it is said is worked out here, apart from the window.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dusk_core::{Command, MediaId, MediaInfo, Project, RelinkMedia};
use dusk_engine::{EngineError, media_info};
use slint::{ComponentHandle, SharedString, VecModel};

use crate::app::{App, file_name, sentence, with_app};
use crate::platform::Dialog;
use crate::shortcuts::Action;
use crate::{MissingView, StatusKind};

/// How often a find says how many of the files beside the picked one it has read.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// What a check of the media files does once it knows which are missing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ask {
    /// Nothing: the media bin shows them.
    #[default]
    No,
    /// A project was opened: say so and list them, when there are any.
    Opened,
    /// An autosave was recovered: as Opened, and say that it is to be saved.
    Recovered,
    /// File → Find missing media: list them, or say that none is missing.
    Asked,
}

/// The checks for missing media files: a check overtaken by a newer one changes nothing, and
/// the newer one still does what the overtaken one was to do.
#[derive(Debug, Default)]
pub struct MediaChecks {
    count: u64,
    /// The newest check done.
    done: u64,
    /// What the check under way is to do.
    pending: Ask,
    /// Whether a check is wanted after the one under way, which may have looked too early.
    again: bool,
}

impl MediaChecks {
    /// A new check, overtaking any under way: its number, and what it is to do, which is `ask`,
    /// or what the overtaken check was to do when `ask` asks nothing.
    pub fn start(&mut self, ask: Ask) -> (u64, Ask) {
        self.count += 1;
        self.again = false;
        if ask != Ask::No {
            self.pending = ask;
        }
        (self.count, self.pending)
    }

    /// Whether `check` is the newest check, which is then done.
    pub fn finish(&mut self, check: u64) -> bool {
        let newest = check == self.count;
        if newest {
            self.done = check;
            self.pending = Ask::No;
        }
        newest
    }

    /// A check is wanted for something that just happened (the engine found a file gone):
    /// true when none is under way, so one is to start now; otherwise one more follows the
    /// check under way, however often this is asked meanwhile.
    pub fn want(&mut self) -> bool {
        if self.done == self.count {
            return true;
        }
        self.again = true;
        false
    }

    /// Whether a check was wanted while the one just done ran; it is then to start.
    pub fn take_again(&mut self) -> bool {
        std::mem::take(&mut self.again)
    }
}

/// The dialog while it is open: which row is picked, and what the last find came to.
pub struct MissingDialog {
    pub picked: usize,
    pub note: String,
    pub kind: StatusKind,
}

impl MissingDialog {
    /// Keeps the pick on one of `rows` rows; false when there are none, and the list is to
    /// close.
    pub fn fit(&mut self, rows: usize) -> bool {
        if rows == 0 {
            return false;
        }
        self.picked = self.picked.min(rows - 1);
        true
    }
}

/// A file Dusk read for a missing media file, and what it holds when Dusk could read it.
pub struct Found {
    pub media: MediaId,
    /// Where the project said the media was when its file was looked for.
    pub was: PathBuf,
    pub path: PathBuf,
    pub info: Option<MediaInfo>,
}

/// What finding a file came to: the edit that relinks the media it fits, and what to say.
pub struct Relinked {
    /// One relink, or several as one step; `None` when nothing fits.
    pub command: Option<Command>,
    pub relinked: Vec<MediaId>,
    pub message: String,
    pub kind: StatusKind,
}

/// What a find for one missing media looks for beside the file picked for it.
#[derive(Debug)]
pub struct Search {
    /// Where the project says the media was.
    pub was: PathBuf,
    /// The other missing media that were in the same folder as it, with where each was: the
    /// files that moved with it. A file of the same name in another folder is another file.
    pub others: Vec<(MediaId, PathBuf)>,
    /// Files no other media may take: the one picked, and those of the media the project finds.
    /// A missing media's own path is not among them, so a file back where it was is found.
    pub taken: Vec<PathBuf>,
}

/// What a find for `media` of `project`, at the file `picked`, looks for beside it, `missing`
/// being the media the project cannot find; `None` when the project has no such media.
pub fn search(
    project: &Project,
    missing: &HashSet<MediaId>,
    media: MediaId,
    picked: &Path,
) -> Option<Search> {
    let was = project.media_ref(media)?.path.clone();
    let folder = |path: &Path| path.parent().map(Path::to_path_buf);
    let others = project
        .media()
        .iter()
        .filter(|other| other.id != media && missing.contains(&other.id))
        .filter(|other| match (folder(&other.path), folder(&was)) {
            (Some(theirs), Some(ours)) => same_path(&theirs, &ours),
            _ => false,
        })
        .map(|other| (other.id, other.path.clone()))
        .collect();
    let taken = std::iter::once(picked.to_path_buf())
        .chain(
            project
                .media()
                .iter()
                .filter(|other| !missing.contains(&other.id))
                .map(|other| other.path.clone()),
        )
        .collect();
    Some(Search { was, others, taken })
}

/// The files among `files` named as the media files `wanted` were, in any case, one for
/// each, with where each media was: the other missing files beside one that was found. A file
/// in `taken` is no other media's, and a name two of `wanted` share could be either's, so it
/// finds neither.
pub fn same_names(
    files: &[PathBuf],
    wanted: &[(MediaId, PathBuf)],
    taken: &[PathBuf],
) -> Vec<(MediaId, PathBuf, PathBuf)> {
    let lowercase = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
    };
    // Each looked up once: how many of `wanted` have a name, the files of each name in order,
    // and the files taken, as the system compares them.
    let mut named: HashMap<String, usize> = HashMap::new();
    for name in wanted.iter().filter_map(|(_, was)| lowercase(was)) {
        *named.entry(name).or_default() += 1;
    }
    let mut files_named: HashMap<String, Vec<&PathBuf>> = HashMap::new();
    for file in files {
        if let Some(name) = lowercase(file) {
            files_named.entry(name).or_default().push(file);
        }
    }
    let taken: HashSet<Vec<String>> = taken.iter().map(|path| path_key(path)).collect();
    wanted
        .iter()
        .filter_map(|(media, was)| {
            let name = lowercase(was).filter(|name| named.get(name) == Some(&1))?;
            let file = files_named
                .get(&name)?
                .iter()
                .find(|file| !taken.contains(&path_key(file)))?;
            Some((*media, was.clone(), (*file).clone()))
        })
        .collect()
}

/// `path` as the system compares file names, part by part: Windows' ignore case.
fn path_key(path: &Path) -> Vec<String> {
    path.components()
        .map(|part| {
            let part = part.as_os_str().to_string_lossy();
            if cfg!(windows) {
                part.to_lowercase()
            } else {
                part.into_owned()
            }
        })
        .collect()
}

/// Whether `a` and `b` name the same file.
fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// Relinks what was found for a missing media file: `found` starts with the file picked for
/// it, then the files of the same names beside it. The picked file must fit (its clips'
/// checks, in `RelinkMedia`) and be no other media's; of the others, those that fit come
/// along in the same step. A media the project no longer names as it did when its file was
/// looked for (another project opened, an edit undone meanwhile) is left as it is.
pub fn relink(project: &Project, found: Vec<Found>) -> Relinked {
    let failed = |message: String| Relinked {
        command: None,
        relinked: Vec::new(),
        message,
        kind: StatusKind::Error,
    };
    let mut found = found.into_iter();
    let Some(picked) = found.next() else {
        return failed(String::new());
    };
    let name = file_name(&picked.path);
    let unchanged = |found: &Found| {
        project
            .media_ref(found.media)
            .is_some_and(|media| media.path == found.was)
    };
    // The media of each file, looked up once a file found.
    let mut users: HashMap<Vec<String>, Vec<MediaId>> = HashMap::new();
    for media in project.media() {
        users
            .entry(path_key(&media.path))
            .or_default()
            .push(media.id);
    }
    let used = |found: &Found| {
        users
            .get(&path_key(&found.path))
            .is_some_and(|ids| ids.iter().any(|id| *id != found.media))
    };
    if !unchanged(&picked) {
        return Relinked {
            command: None,
            relinked: Vec::new(),
            message: format!(
                "The project changed while Dusk read {name}; if a file is still missing, find \
                 it again."
            ),
            kind: StatusKind::Warning,
        };
    }
    if used(&picked) {
        return failed(format!(
            "{name} is already in the project as another media file. Pick the file the project \
             used, or a copy of it."
        ));
    }
    // Why it could not be read does not matter here: its reader's advice (convert it, import
    // something else) is not what a find needs, which is the file the project used.
    let Some(info) = picked.info else {
        return failed(format!(
            "Dusk could not read {name}; pick the file the project used, or a copy of it."
        ));
    };
    // Each relink is tried on a copy, after the ones before it, as the step will apply them.
    let mut scratch = project.clone();
    let mut first = Command::RelinkMedia(RelinkMedia::new(picked.media, picked.path, info));
    if let Err(rejection) = first.apply(&mut scratch) {
        return failed(sentence(&rejection.to_string()));
    }
    let (mut commands, mut relinked, mut misfits) = (vec![first], vec![picked.media], 0);
    for other in found.filter(|other| unchanged(other) && !used(other)) {
        let Some(info) = other.info else {
            misfits += 1;
            continue;
        };
        let mut command = Command::RelinkMedia(RelinkMedia::new(other.media, other.path, info));
        if command.apply(&mut scratch).is_ok() {
            commands.push(command);
            relinked.push(other.media);
        } else {
            misfits += 1;
        }
    }
    let mut message = match relinked.len() - 1 {
        0 => format!("Found {name}."),
        more => format!("Found {name}, and {more} more beside it."),
    };
    match misfits {
        0 => {}
        1 => message.push_str(
            " 1 file beside it has the name of another missing file, but is not the same file.",
        ),
        more => message.push_str(&format!(
            " {more} files beside it have the names of other missing files, but are not the \
             same files."
        )),
    }
    let command = if commands.len() == 1 {
        commands.pop()
    } else {
        Some(Command::Batch(commands))
    };
    Relinked {
        command,
        relinked,
        message,
        kind: if misfits == 0 {
            StatusKind::Info
        } else {
            StatusKind::Warning
        },
    }
}

impl App {
    /// Checks on a thread of its own which of the project's media files are not where it
    /// says, since a drive that went away can keep the system waiting; then does what `ask`
    /// says. A newer check overtakes one under way, and does what that one was to do too.
    pub(crate) fn check_media(&mut self, ask: Ask) {
        let (check, ask) = self.media_checks.start(ask);
        let paths: Vec<(MediaId, PathBuf)> = self
            .project
            .media()
            .iter()
            .map(|media| (media.id, media.path.clone()))
            .collect();
        let spawned = std::thread::Builder::new()
            .name("dusk media check".to_owned())
            .spawn(move || {
                let missing: Vec<MediaId> = paths
                    .into_iter()
                    .filter(|(_, path)| !path.exists())
                    .map(|(media, _)| media)
                    .collect();
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| app.media_checked(check, missing, ask));
                });
            });
        if let Err(error) = spawned {
            // Done without looking, so the next check the engine asks for can start.
            self.media_checks.finish(check);
            self.fail(&sentence(&EngineError::Thread(error).to_string()));
        }
    }

    fn media_checked(&mut self, check: u64, missing: Vec<MediaId>, ask: Ask) {
        if !self.media_checks.finish(check) {
            return;
        }
        let missing: HashSet<MediaId> = missing.into_iter().collect();
        // A file back where it was gets its thumbnail asked for again: none could be made
        // while it was away.
        for media in self.missing.difference(&missing) {
            self.thumbnails.forget(*media);
        }
        self.missing = missing;
        self.refresh_bin();
        match ask {
            Ask::No => {}
            Ask::Opened | Ask::Recovered if self.missing.is_empty() => {}
            Ask::Opened | Ask::Recovered => {
                let count = self.missing.len();
                let lost = if count == 1 {
                    "A media file of this project cannot be found, so its clips stay black and \
                     silent; find it in the list."
                        .to_owned()
                } else {
                    format!(
                        "{count} media files of this project cannot be found, so their clips \
                         stay black and silent; find them in the list."
                    )
                };
                self.fail(&if ask == Ask::Recovered {
                    format!("Recovered; save to keep it. {lost}")
                } else {
                    lost
                });
                self.list_missing();
            }
            Ask::Asked if self.missing.is_empty() => {
                self.say("Every media file is where the project says.");
            }
            Ask::Asked => self.list_missing(),
        }
        self.fit_missing_dialog();
        // The engine found a file gone while this check ran, which may have looked before.
        if self.media_checks.take_again() {
            self.check_media(Ask::No);
        }
    }

    /// Opens the list of missing files, or, while a question or another dialog is open, once
    /// it has been answered.
    fn list_missing(&mut self) {
        if self.missing_dialog.is_none() && self.main_dialog_open() {
            self.missing_waiting = true;
        } else {
            self.open_missing_dialog();
        }
    }

    /// The last dialog or question over the window closed: the list that waited for it
    /// opens, while files are missing.
    pub(crate) fn missing_list_waited(&mut self) {
        if std::mem::take(&mut self.missing_waiting) && !self.missing.is_empty() {
            self.list_missing();
        }
    }

    /// Keeps an open list in step with what is missing: its pick on a row, and closed once
    /// nothing is.
    fn fit_missing_dialog(&mut self) {
        let rows = self.missing_media().len();
        let Some(dialog) = &mut self.missing_dialog else {
            return;
        };
        if dialog.fit(rows) {
            self.refresh_missing_dialog();
        } else {
            self.end_missing_list();
        }
    }

    /// File → Find missing media and its key: the list, once a check has said what is
    /// missing now.
    pub(crate) fn find_missing_media(&mut self) {
        if self.main_dialog_open() {
            return;
        }
        self.check_media(Ask::Asked);
    }

    fn open_missing_dialog(&mut self) {
        if self.missing_dialog.is_none() && !self.main_dialog_open() {
            self.missing_dialog = Some(MissingDialog {
                picked: 0,
                note: String::new(),
                kind: StatusKind::Info,
            });
        }
        self.refresh_missing_dialog();
    }

    /// The missing media, in the project's order.
    fn missing_media(&self) -> Vec<(MediaId, PathBuf)> {
        self.project
            .media()
            .iter()
            .filter(|media| self.missing.contains(&media.id))
            .map(|media| (media.id, media.path.clone()))
            .collect()
    }

    pub fn missing_pick(&mut self, row: i32) {
        let rows = self.missing_media().len();
        if let (Some(dialog), Ok(row)) = (&mut self.missing_dialog, usize::try_from(row))
            && row < rows
        {
            dialog.picked = row;
            self.show_missing_pick();
        }
    }

    /// Shows which row is picked, without listing the rows again.
    fn show_missing_pick(&self) {
        if let (Some(dialog), Some(window)) = (&self.missing_dialog, self.window()) {
            window.set_missing_picked(i32::try_from(dialog.picked).unwrap_or(0));
        }
    }

    /// Find…: asks for the picked file with the system's dialog, in its old folder when that
    /// is still there.
    pub fn missing_find(&mut self) {
        let Some(dialog) = &self.missing_dialog else {
            return;
        };
        let Some((media, was)) = self.missing_media().get(dialog.picked).cloned() else {
            return;
        };
        let Some(window) = self.window() else {
            return;
        };
        let ask = Dialog::FindMedia {
            name: file_name(&was),
            folder: was.parent().map(Path::to_path_buf),
        };
        self.show_dialog_over(window.window(), ask, move |paths| {
            if let Some(path) = paths.into_iter().next() {
                with_app(|app| app.find_media(media, path));
            }
        });
    }

    /// Reads the file picked for `media` on a thread of its own, then the files beside it
    /// named as the other missing media from its old folder, and relinks what fits. It says
    /// how far it has got; closing the list, or another find, stops it.
    fn find_media(&mut self, media: MediaId, path: PathBuf) {
        let Some(search) = search(&self.project, &self.missing, media, &path) else {
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(earlier) = self.relink.replace(Arc::clone(&stop)) {
            earlier.store(true, Ordering::Relaxed);
        }
        let name = file_name(&path);
        self.say(&format!("Reading {name}…"));
        let spawned = std::thread::Builder::new()
            .name("dusk relink".to_owned())
            .spawn(move || {
                let read = |path: &Path| media_info(path).ok();
                let Search { was, others, taken } = search;
                let mut found = vec![Found {
                    media,
                    was,
                    info: read(&path),
                    path: path.clone(),
                }];
                let beside: Vec<PathBuf> = path
                    .parent()
                    .and_then(|folder| std::fs::read_dir(folder).ok())
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| Some(entry.ok()?.path()))
                    .collect();
                let matches = same_names(&beside, &others, &taken);
                let total = matches.len();
                let mut said = Instant::now();
                for (done, (other, was, file)) in matches.into_iter().enumerate() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    found.push(Found {
                        media: other,
                        was,
                        info: read(&file),
                        path: file,
                    });
                    if said.elapsed() >= PROGRESS_EVERY {
                        said = Instant::now();
                        let stop = Arc::clone(&stop);
                        let message =
                            format!("Reading the files beside {name}: {} of {total}…", done + 1);
                        let _ = slint::invoke_from_event_loop(move || {
                            if !stop.load(Ordering::Relaxed) {
                                with_app(|app| app.say(&message));
                            }
                        });
                    }
                }
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| app.media_found(found, &stop));
                });
            });
        if let Err(error) = spawned {
            self.relink = None;
            self.fail(&sentence(&EngineError::Thread(error).to_string()));
        }
    }

    fn media_found(&mut self, found: Vec<Found>, stop: &AtomicBool) {
        // The list closed, or a newer find started: this one is let go.
        if stop.load(Ordering::Relaxed) {
            return;
        }
        self.relink = None;
        let relinked = relink(&self.project, found);
        if let Some(command) = relinked.command
            && self.edit(command)
        {
            for media in &relinked.relinked {
                self.missing.remove(media);
                // Its thumbnail could not be made while its file was away.
                self.thumbnails.forget(*media);
            }
            self.refresh_bin();
        }
        self.status(&relinked.message, relinked.kind);
        if let Some(dialog) = &mut self.missing_dialog {
            dialog.note = relinked.message;
            dialog.kind = relinked.kind;
        }
        self.fit_missing_dialog();
    }

    /// Close, or Escape: the list closes, and a find under way stops.
    pub fn missing_close(&mut self) {
        if self.end_missing_list() {
            self.say("Stopped reading the files found.");
        }
    }

    /// Closes the list, and stops a find under way, since what it would find was for the
    /// list; true when one was stopped.
    pub(crate) fn end_missing_list(&mut self) -> bool {
        self.missing_dialog = None;
        self.missing_waiting = false;
        if let Some(window) = self.window() {
            window.set_missing_open(false);
        }
        let stop = self.relink.take();
        if let Some(stop) = &stop {
            stop.store(true, Ordering::Relaxed);
        }
        stop.is_some()
    }

    /// Up and Down pick a file, Enter finds it and Escape closes the list; true when the
    /// list is open, which then takes every key.
    pub(crate) fn missing_dialog_key(&mut self, text: &str) -> bool {
        use slint::platform::Key;
        let rows = self.missing_media().len();
        let Some(dialog) = &mut self.missing_dialog else {
            return false;
        };
        let named = |key: Key| text == SharedString::from(key).as_str();
        if named(Key::UpArrow) {
            dialog.picked = dialog.picked.saturating_sub(1);
        } else if named(Key::DownArrow) {
            dialog.picked = (dialog.picked + 1).min(rows.saturating_sub(1));
        } else {
            if named(Key::Return) {
                self.missing_find();
            } else if named(Key::Escape) {
                self.missing_close();
            }
            return true;
        }
        self.show_missing_pick();
        true
    }

    /// Shows the list as it stands, while it is open.
    pub(crate) fn refresh_missing_dialog(&self) {
        let (Some(dialog), Some(window)) = (&self.missing_dialog, self.window()) else {
            return;
        };
        let rows: Vec<MissingView> = self
            .missing_media()
            .iter()
            .map(|(_, was)| MissingView {
                name: file_name(was).into(),
                folder: was
                    .parent()
                    .map(|folder| folder.display().to_string())
                    .unwrap_or_default()
                    .into(),
            })
            .collect();
        window.set_missing_rows(Rc::new(VecModel::from(rows)).into());
        window.set_missing_picked(i32::try_from(dialog.picked).unwrap_or(0));
        window.set_missing_note(dialog.note.clone().into());
        window.set_missing_note_kind(dialog.kind);
        window.set_missing_open(true);
    }

    /// What the media bin says of a missing file: how to find it.
    pub(crate) fn missing_detail(&self) -> String {
        format!("Cannot be found: {}", self.find_hint())
    }

    /// How to find a missing file: the key that does it, or the menu item when the user took
    /// its keys away.
    fn find_hint(&self) -> String {
        let keys = self.keymap.keys_text(Action::FindMissingMedia);
        if keys.is_empty() {
            "File, Find missing media finds it".to_owned()
        } else {
            format!("{keys} finds it")
        }
    }

    /// The engine could not do something. A media file that went away while Dusk ran says
    /// so and how to find it, and the bin marks it once a check has seen it gone.
    pub(crate) fn engine_failed(&mut self, error: &EngineError) {
        if let Some(path) = error.missing_file()
            && let Some(message) = gone_message(&self.project, path, &self.find_hint())
        {
            let known = self
                .project
                .media()
                .iter()
                .any(|media| media.path == path && self.missing.contains(&media.id));
            // One check at a time: a drive that went away keeps each waiting, while the
            // engine reports the file again at every frame it cannot show.
            if !known && self.media_checks.want() {
                self.check_media(Ask::No);
            }
            return self.fail(&message);
        }
        self.fail(&sentence(&error.to_string()));
    }
}

/// The media of `project` and where each is, to tell when an edit moved one.
pub(crate) fn media_paths(project: &Project) -> Vec<(MediaId, &Path)> {
    project
        .media()
        .iter()
        .map(|media| (media.id, media.path.as_path()))
        .collect()
}

/// What the status line says when the engine finds that `path`, a media file of `project`,
/// went away; `hint` says how to find it. `None` for a file the project does not use.
pub fn gone_message(project: &Project, path: &Path, hint: &str) -> Option<String> {
    let media = project.media().iter().find(|media| media.path == path)?;
    Some(format!(
        "{} cannot be found, so its clips are black and silent; {hint}.",
        file_name(&media.path)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{Frame, MediaKind, MediaTime, Orientation, Rational, add_media, import};
    use std::collections::HashSet;

    fn info(seconds: i64) -> MediaInfo {
        MediaInfo {
            kind: MediaKind::Video,
            duration: MediaTime(seconds * 1_000_000),
            has_video: true,
            has_audio: true,
            frame_rate: Some(Rational::new(30, 1).unwrap()),
            vfr: false,
            width: 1920,
            height: 1080,
            orientation: Orientation::UPRIGHT,
        }
    }

    /// A project with two 10 s clips from E:/trip, both missing.
    fn project() -> (Project, MediaId, MediaId) {
        let mut project = Project::new(Rational::new(30, 1).unwrap(), (1920, 1080));
        for (name, at) in [("beach.mp4", 0), ("hills.mp4", 300)] {
            import(
                &project,
                format!("E:/trip/{name}").into(),
                info(10),
                Frame(at),
            )
            .apply(&mut project)
            .unwrap();
        }
        let ids: Vec<MediaId> = project.media().iter().map(|media| media.id).collect();
        (project, ids[0], ids[1])
    }

    /// What was read for `media`, still named as the project names it.
    fn found(media: MediaId, path: &str, info: Option<MediaInfo>) -> Found {
        let (project, _, _) = project();
        Found {
            media,
            was: project.media_ref(media).unwrap().path.clone(),
            path: path.into(),
            info,
        }
    }

    fn paths(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn the_picked_file_is_not_found_again_for_a_missing_file_of_its_name() {
        // Cameras name files the same on every card: day 2's C0001 is not day 1's.
        let files = paths(&["F:/day1/C0001.MP4", "F:/day1/C0002.MP4"]);
        let wanted = vec![
            (MediaId(2), PathBuf::from("E:/day2/C0001.MP4")),
            (MediaId(3), PathBuf::from("E:/day1/C0002.MP4")),
        ];
        let others = [(
            MediaId(3),
            PathBuf::from("E:/day1/C0002.MP4"),
            PathBuf::from("F:/day1/C0002.MP4"),
        )];
        assert_eq!(
            same_names(&files, &wanted, &paths(&["F:/day1/C0001.MP4"])),
            others
        );
        // Windows' file names ignore case, so the picked file is taken in any case there.
        if cfg!(windows) {
            assert_eq!(
                same_names(&files, &wanted, &paths(&["F:/day1/c0001.mp4"])),
                others
            );
        }
    }

    #[test]
    fn a_name_two_missing_files_share_finds_neither() {
        let files = paths(&["F:/cards/C0001.MP4"]);
        let wanted = vec![
            (MediaId(2), PathBuf::from("E:/day2/C0001.MP4")),
            (MediaId(3), PathBuf::from("E:/day3/c0001.mp4")),
        ];
        assert_eq!(same_names(&files, &wanted, &[]), []);
    }

    #[test]
    fn a_file_the_project_uses_is_not_found_for_another_media() {
        let files = paths(&["F:/trip/beach.mp4"]);
        let wanted = vec![(MediaId(2), PathBuf::from("E:/old/beach.mp4"))];
        assert_eq!(
            same_names(&files, &wanted, &paths(&["F:/trip/beach.mp4"])),
            []
        );
    }

    /// A project of a 10 s video at each of `paths`, and their ids.
    fn project_of(paths: &[&str]) -> (Project, Vec<MediaId>) {
        let mut project = Project::new(Rational::new(30, 1).unwrap(), (1920, 1080));
        let ids = paths
            .iter()
            .map(|path| {
                let (id, mut command) = add_media(&project, (*path).into(), info(10));
                command.apply(&mut project).unwrap();
                id
            })
            .collect();
        (project, ids)
    }

    #[test]
    fn only_missing_files_from_the_picked_ones_old_folder_are_looked_for() {
        // Two cards, each with its own C0001..C0009: card 2's file is not looked for in the
        // folder of card 1's, where a file of its name is another shot.
        let (project, ids) = project_of(&[
            "E:/card1/C0001.MP4",
            "E:/card2/C0002.MP4",
            "E:/card1/C0003.MP4",
            "F:/music/song.m4a",
        ]);
        let missing: HashSet<MediaId> = ids[..3].iter().copied().collect();
        let search = search(&project, &missing, ids[0], Path::new("F:/card1/C0001.MP4")).unwrap();
        assert_eq!(search.was, PathBuf::from("E:/card1/C0001.MP4"));
        assert_eq!(
            search.others,
            [(ids[2], PathBuf::from("E:/card1/C0003.MP4"))]
        );
        // The picked file and those of the media the project finds are no one else's.
        assert_eq!(
            search.taken,
            paths(&["F:/card1/C0001.MP4", "F:/music/song.m4a"])
        );
    }

    #[test]
    fn files_back_at_their_own_paths_are_found_beside_the_picked_one() {
        // A drive that came back: the files are where the project says.
        let (project, ids) = project_of(&["E:/trip/a.mp4", "E:/trip/b.mp4"]);
        let missing: HashSet<MediaId> = ids.iter().copied().collect();
        let search = search(&project, &missing, ids[0], Path::new("E:/trip/a.mp4")).unwrap();
        let files = paths(&["E:/trip/a.mp4", "E:/trip/b.mp4"]);
        assert_eq!(
            same_names(&files, &search.others, &search.taken),
            [(
                ids[1],
                PathBuf::from("E:/trip/b.mp4"),
                PathBuf::from("E:/trip/b.mp4")
            )]
        );
    }

    #[test]
    fn a_picked_file_another_media_uses_relinks_nothing() {
        let (project, beach, _) = project();
        // hills.mp4 is where the project says; picked for beach.mp4 it would play twice.
        let relinked = relink(
            &project,
            vec![found(beach, "E:/trip/hills.mp4", Some(info(10)))],
        );
        assert!(relinked.command.is_none());
        assert_eq!(relinked.kind, StatusKind::Error);
        assert_eq!(
            relinked.message,
            "hills.mp4 is already in the project as another media file. Pick the file the \
             project used, or a copy of it."
        );
    }

    #[test]
    fn a_media_that_changed_while_its_file_was_read_is_left_alone() {
        // Another project was opened meanwhile, whose media has the same id.
        let (project, beach, hills) = project();
        let mut asked = found(beach, "F:/trip/beach.mp4", Some(info(10)));
        asked.was = "E:/other project/beach.mp4".into();
        let relinked = relink(
            &project,
            vec![asked, found(hills, "F:/trip/hills.mp4", Some(info(10)))],
        );
        assert!(relinked.command.is_none());
        assert_eq!(relinked.kind, StatusKind::Warning);
        assert_eq!(
            relinked.message,
            "The project changed while Dusk read beach.mp4; if a file is still missing, find \
             it again."
        );
        // A file beside it whose media changed meanwhile is left too.
        let mut beside = found(hills, "F:/trip/hills.mp4", Some(info(10)));
        beside.was = "E:/other project/hills.mp4".into();
        let relinked = relink(
            &project,
            vec![found(beach, "F:/trip/beach.mp4", Some(info(10))), beside],
        );
        assert_eq!(relinked.relinked, [beach]);
        assert_eq!(relinked.message, "Found beach.mp4.");
    }

    #[test]
    fn the_list_keeps_its_pick_on_a_row_and_closes_once_nothing_is_missing() {
        let mut dialog = MissingDialog {
            picked: 2,
            note: String::new(),
            kind: StatusKind::Info,
        };
        // A check found the third file back: the pick moves onto the last row left.
        assert!(dialog.fit(2));
        assert_eq!(dialog.picked, 1);
        assert!(dialog.fit(2));
        assert_eq!(dialog.picked, 1);
        // Every file is back: the list closes.
        assert!(!dialog.fit(0));
    }

    #[test]
    fn reports_while_a_check_is_under_way_ask_for_one_more_after_it() {
        // A drive that went away keeps a check waiting, while the engine reports the file
        // gone again and again: one check runs at a time, and one more follows it.
        let mut checks = MediaChecks::default();
        assert!(checks.want());
        let (first, _) = checks.start(Ask::No);
        assert!(!checks.want());
        assert!(!checks.want());
        assert!(checks.finish(first));
        assert!(checks.take_again());
        assert!(!checks.take_again());
        // A check that starts after the reports looks after them.
        let (second, _) = checks.start(Ask::No);
        assert!(!checks.want());
        let (third, _) = checks.start(Ask::Asked);
        assert!(!checks.finish(second));
        assert!(checks.finish(third));
        assert!(!checks.take_again());
        assert!(checks.want());
    }

    #[test]
    fn a_check_that_overtakes_one_still_does_what_that_one_was_to_do() {
        // A project was opened, so its missing files are to be listed; the engine's report of
        // one of them starts a newer check before the first comes back.
        let mut checks = MediaChecks::default();
        let (opened, _) = checks.start(Ask::Opened);
        let (newer, ask) = checks.start(Ask::No);
        assert_eq!(ask, Ask::Opened);
        assert!(!checks.finish(opened));
        assert!(checks.finish(newer));
        // Done: the next check does only what it is asked.
        assert_eq!(checks.start(Ask::No).1, Ask::No);
    }

    #[test]
    fn a_media_file_that_went_away_says_how_to_find_it() {
        let (project, _, _) = project();
        assert_eq!(
            gone_message(
                &project,
                Path::new("E:/trip/hills.mp4"),
                "Ctrl+Shift+M finds it"
            )
            .as_deref(),
            Some(
                "hills.mp4 cannot be found, so its clips are black and silent; Ctrl+Shift+M \
                 finds it."
            )
        );
        // Files the project does not use are not its media.
        assert_eq!(
            gone_message(
                &project,
                Path::new("E:/trip/notes.mp4"),
                "Ctrl+Shift+M finds it"
            ),
            None
        );
    }

    #[test]
    fn files_beside_the_found_one_are_matched_by_name_in_any_case() {
        let files: Vec<PathBuf> = [
            "F:/trip/BEACH.MP4",
            "F:/trip/Hills.mp4",
            "F:/trip/notes.txt",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let wanted = vec![
            (MediaId(1), PathBuf::from("E:/trip/beach.mp4")),
            (MediaId(2), PathBuf::from("E:/trip/hills.mp4")),
            (MediaId(3), PathBuf::from("E:/other/song.mp3")),
        ];
        assert_eq!(
            same_names(&files, &wanted, &[]),
            [
                (
                    MediaId(1),
                    PathBuf::from("E:/trip/beach.mp4"),
                    PathBuf::from("F:/trip/BEACH.MP4")
                ),
                (
                    MediaId(2),
                    PathBuf::from("E:/trip/hills.mp4"),
                    PathBuf::from("F:/trip/Hills.mp4")
                ),
            ]
        );
    }

    #[test]
    fn a_found_file_relinks_with_the_ones_beside_it_in_one_step() {
        let (mut project, beach, hills) = project();
        let relinked = relink(
            &project,
            vec![
                found(beach, "F:/trip/beach.mp4", Some(info(10))),
                found(hills, "F:/trip/hills.mp4", Some(info(10))),
            ],
        );
        assert_eq!(relinked.relinked, [beach, hills]);
        assert_eq!(relinked.kind, StatusKind::Info);
        assert_eq!(relinked.message, "Found beach.mp4, and 1 more beside it.");
        let mut command = relinked.command.expect("an edit");
        command.apply(&mut project).unwrap();
        assert_eq!(
            project.media_ref(hills).unwrap().path,
            Path::new("F:/trip/hills.mp4")
        );
        // One step: undoing it puts both back.
        command.revert(&mut project);
        assert_eq!(
            project.media_ref(beach).unwrap().path,
            Path::new("E:/trip/beach.mp4")
        );
    }

    #[test]
    fn a_file_beside_it_that_does_not_fit_is_left_and_counted() {
        let (project, beach, hills) = project();
        let relinked = relink(
            &project,
            vec![
                found(beach, "F:/trip/beach.mp4", Some(info(10))),
                // Shorter than its clip.
                found(hills, "F:/trip/hills.mp4", Some(info(2))),
            ],
        );
        assert_eq!(relinked.relinked, [beach]);
        assert_eq!(relinked.kind, StatusKind::Warning);
        assert_eq!(
            relinked.message,
            "Found beach.mp4. 1 file beside it has the name of another missing file, but is not \
             the same file."
        );
    }

    #[test]
    fn a_picked_file_that_does_not_fit_relinks_nothing() {
        let (project, beach, hills) = project();
        let relinked = relink(
            &project,
            vec![
                found(beach, "F:/trip/beach.mp4", Some(info(2))),
                found(hills, "F:/trip/hills.mp4", Some(info(10))),
            ],
        );
        assert!(relinked.command.is_none() && relinked.relinked.is_empty());
        assert_eq!(relinked.kind, StatusKind::Error);
        assert_eq!(
            relinked.message,
            "That file is shorter than the clips that use it; pick the file the project used, \
             or a copy of it."
        );
    }

    #[test]
    fn a_picked_file_dusk_cannot_read_asks_only_for_the_right_file() {
        // The reader's own advice (convert it, import something else) is not what a find
        // needs: the file the project used.
        let (project, beach, _) = project();
        let relinked = relink(&project, vec![found(beach, "F:/trip/beach.mp4", None)]);
        assert!(relinked.command.is_none());
        assert_eq!(relinked.kind, StatusKind::Error);
        assert_eq!(
            relinked.message,
            "Dusk could not read beach.mp4; pick the file the project used, or a copy of it."
        );
    }
}
