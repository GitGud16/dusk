//! Media files a project cannot find (docs/ARCHITECTURE.md, "Release (0.1)"): a dialog lists
//! them, and finding one relinks it, with the other missing files that lie beside it, as one
//! undoable edit. What is found and how it is said is worked out here, apart from the window.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use dusk_core::{Command, MediaId, MediaInfo, Project, RelinkMedia};
use dusk_engine::{EngineError, media_info};
use slint::{ComponentHandle, SharedString, VecModel};

use crate::app::{App, sentence, with_app};
use crate::platform::Dialog;
use crate::shortcuts::Action;
use crate::{MissingView, StatusKind};

/// What a check of the media files does once it knows which are missing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ask {
    /// Nothing: the media bin shows them.
    #[default]
    No,
    /// A project was opened: say so and list them, when there are any.
    Opened,
    /// File → Find missing media: list them, or say that none is missing.
    Asked,
}

/// The dialog while it is open: which row is picked, and what the last find came to.
pub struct MissingDialog {
    pub picked: usize,
    pub note: String,
    pub kind: StatusKind,
}

/// A file Dusk read for a missing media file: what it holds, or why it could not be read.
pub struct Found {
    pub media: MediaId,
    pub path: PathBuf,
    pub info: Result<MediaInfo, String>,
}

/// What finding a file came to: the edit that relinks the media it fits, and what to say.
pub struct Relinked {
    /// One relink, or several as one step; `None` when nothing fits.
    pub command: Option<Command>,
    pub relinked: Vec<MediaId>,
    pub message: String,
    pub kind: StatusKind,
}

/// The files among `files` named as the media files `wanted` were, in any case, one for
/// each: the other missing files beside one that was found.
pub fn same_names(files: &[PathBuf], wanted: &[(MediaId, PathBuf)]) -> Vec<(MediaId, PathBuf)> {
    let lowercase = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
    };
    wanted
        .iter()
        .filter_map(|(media, was)| {
            let name = lowercase(was)?;
            let file = files
                .iter()
                .find(|file| lowercase(file).as_ref() == Some(&name))?;
            Some((*media, file.clone()))
        })
        .collect()
}

/// Relinks what was found for a missing media file: `found` starts with the file picked for
/// it, then the files of the same names beside it. The picked file must fit (its clips'
/// checks, in `RelinkMedia`); of the others, those that fit come along in the same step.
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
    let info = match picked.info {
        Ok(info) => info,
        Err(error) => {
            return failed(format!(
                "Dusk could not read {name}: {error}. Pick the file the project used, or a copy \
                 of it."
            ));
        }
    };
    // Each relink is tried on a copy, after the ones before it, as the step will apply them.
    let mut scratch = project.clone();
    let mut first = Command::RelinkMedia(RelinkMedia::new(picked.media, picked.path, info));
    if let Err(rejection) = first.apply(&mut scratch) {
        return failed(sentence(&rejection.to_string()));
    }
    let (mut commands, mut relinked, mut misfits) = (vec![first], vec![picked.media], 0);
    for other in found {
        let Ok(info) = other.info else {
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
    /// says. A newer check overtakes one under way.
    pub(crate) fn check_media(&mut self, ask: Ask) {
        self.media_check += 1;
        let check = self.media_check;
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
            self.fail(&sentence(&EngineError::Thread(error).to_string()));
        }
    }

    fn media_checked(&mut self, check: u64, missing: Vec<MediaId>, ask: Ask) {
        if check != self.media_check {
            return;
        }
        self.missing = missing.into_iter().collect();
        self.refresh_bin();
        match ask {
            Ask::No => {}
            Ask::Opened if self.missing.is_empty() => {}
            Ask::Opened => {
                let count = self.missing.len();
                self.fail(&if count == 1 {
                    "A media file of this project cannot be found, so its clips stay black and \
                     silent; find it in the list."
                        .to_owned()
                } else {
                    format!(
                        "{count} media files of this project cannot be found, so their clips \
                         stay black and silent; find them in the list."
                    )
                });
                self.open_missing_dialog();
            }
            Ask::Asked if self.missing.is_empty() => {
                self.say("Every media file is where the project says.");
            }
            Ask::Asked => self.open_missing_dialog(),
        }
        self.refresh_missing_dialog();
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
        let rows = self.missing.len();
        if let (Some(dialog), Ok(row)) = (&mut self.missing_dialog, usize::try_from(row))
            && row < rows
        {
            dialog.picked = row;
        }
        self.refresh_missing_dialog();
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

    /// Reads the file picked for `media` on a thread of its own, and the files beside it
    /// named as the other missing ones, then relinks what fits.
    fn find_media(&mut self, media: MediaId, path: PathBuf) {
        let others: Vec<(MediaId, PathBuf)> = self
            .missing_media()
            .into_iter()
            .filter(|(other, _)| *other != media)
            .collect();
        self.say(&format!("Reading {}…", file_name(&path)));
        let spawned = std::thread::Builder::new()
            .name("dusk relink".to_owned())
            .spawn(move || {
                let read = |path: &Path| media_info(path).map_err(|error| error.to_string());
                let mut found = vec![Found {
                    media,
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
                for (other, file) in same_names(&beside, &others) {
                    found.push(Found {
                        media: other,
                        info: read(&file),
                        path: file,
                    });
                }
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| app.media_found(found));
                });
            });
        if let Err(error) = spawned {
            self.fail(&sentence(&EngineError::Thread(error).to_string()));
        }
    }

    fn media_found(&mut self, found: Vec<Found>) {
        let relinked = relink(&self.project, found);
        if let Some(command) = relinked.command
            && self.edit(command)
        {
            for media in &relinked.relinked {
                self.missing.remove(media);
                self.thumbnails.forget(*media);
            }
            self.refresh_bin();
        }
        match relinked.kind {
            StatusKind::Error => self.fail(&relinked.message),
            StatusKind::Warning => self.warn(&relinked.message),
            _ => self.say(&relinked.message),
        }
        if self.missing.is_empty() {
            return self.missing_close();
        }
        if let Some(dialog) = &mut self.missing_dialog {
            dialog.note = relinked.message;
            dialog.kind = relinked.kind;
            dialog.picked = dialog.picked.min(self.missing.len() - 1);
        }
        self.refresh_missing_dialog();
    }

    pub fn missing_close(&mut self) {
        self.missing_dialog = None;
        if let Some(window) = self.window() {
            window.set_missing_open(false);
        }
    }

    /// Up and Down pick a file, Enter finds it and Escape closes the list; true when the
    /// list is open, which then takes every key.
    pub(crate) fn missing_dialog_key(&mut self, text: &str) -> bool {
        use slint::platform::Key;
        let rows = self.missing.len();
        let Some(dialog) = &mut self.missing_dialog else {
            return false;
        };
        let named = |key: Key| text == SharedString::from(key).as_str();
        if named(Key::UpArrow) {
            dialog.picked = dialog.picked.saturating_sub(1);
        } else if named(Key::DownArrow) {
            dialog.picked = (dialog.picked + 1).min(rows.saturating_sub(1));
        } else if named(Key::Return) {
            self.missing_find();
        } else if named(Key::Escape) {
            self.missing_close();
        }
        self.refresh_missing_dialog();
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
            if !known {
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

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{Frame, MediaKind, MediaTime, Orientation, Rational, import};

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

    fn found(media: MediaId, path: &str, info: Result<MediaInfo, String>) -> Found {
        Found {
            media,
            path: path.into(),
            info,
        }
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
            same_names(&files, &wanted),
            [
                (MediaId(1), PathBuf::from("F:/trip/BEACH.MP4")),
                (MediaId(2), PathBuf::from("F:/trip/Hills.mp4")),
            ]
        );
    }

    #[test]
    fn a_found_file_relinks_with_the_ones_beside_it_in_one_step() {
        let (mut project, beach, hills) = project();
        let relinked = relink(
            &project,
            vec![
                found(beach, "F:/trip/beach.mp4", Ok(info(10))),
                found(hills, "F:/trip/hills.mp4", Ok(info(10))),
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
                found(beach, "F:/trip/beach.mp4", Ok(info(10))),
                // Shorter than its clip.
                found(hills, "F:/trip/hills.mp4", Ok(info(2))),
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
                found(beach, "F:/trip/beach.mp4", Ok(info(2))),
                found(hills, "F:/trip/hills.mp4", Ok(info(10))),
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
    fn a_picked_file_dusk_cannot_read_says_why() {
        let (project, beach, _) = project();
        let relinked = relink(
            &project,
            vec![found(
                beach,
                "F:/trip/beach.mp4",
                Err("it is not a media file".into()),
            )],
        );
        assert!(relinked.command.is_none());
        assert_eq!(relinked.kind, StatusKind::Error);
        assert_eq!(
            relinked.message,
            "Dusk could not read beach.mp4: it is not a media file. Pick the file the project \
             used, or a copy of it."
        );
    }
}
