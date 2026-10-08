//! Autosave and crash recovery (docs/ARCHITECTURE.md, "Project file and autosave"). Every
//! running Dusk keeps a session in the autosave folder: a record naming the project file it
//! edits, locked for as long as that Dusk runs, and the latest autosave of the project. A
//! record nobody holds belongs to a Dusk that stopped without closing its project, and that
//! session's autosave is offered for recovery.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// The extension of a session record.
const RECORD: &str = "session";
/// The extension of an autosave.
const AUTOSAVE: &str = "dusk.autosave";

/// This Dusk's session.
pub struct Session {
    folder: PathBuf,
    name: String,
    /// The record, locked while the session lasts.
    record: File,
}

impl Session {
    /// Starts a session in `folder`, making the folder if needed.
    pub fn start(folder: &Path) -> io::Result<Session> {
        static STARTED: AtomicU64 = AtomicU64::new(0);
        std::fs::create_dir_all(folder)?;
        let since = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let name = format!(
            "{}-{}-{}",
            std::process::id(),
            since.as_millis(),
            STARTED.fetch_add(1, Ordering::Relaxed)
        );
        let record = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(folder.join(format!("{name}.{RECORD}")))?;
        // Held until the record is dropped, which the system does for a Dusk that crashes.
        record.lock()?;
        Ok(Session {
            folder: folder.to_path_buf(),
            name,
            record,
        })
    }

    /// Records that the session edits the project file `project`, or an untitled project.
    pub fn record_project(&mut self, project: Option<&Path>) -> io::Result<()> {
        let text = project.map_or_else(String::new, |path| path.to_string_lossy().into_owned());
        self.record.set_len(0)?;
        self.record.seek(SeekFrom::Start(0))?;
        self.record.write_all(text.as_bytes())
    }

    /// Where the session's autosave goes.
    pub fn autosave(&self) -> PathBuf {
        self.folder.join(format!("{}.{AUTOSAVE}", self.name))
    }

    /// Ends the session with its project closed: its files go. A session that is dropped
    /// without this, as when Dusk crashes, leaves its autosave for recovery.
    pub fn close(self) -> io::Result<()> {
        // The autosave goes first, so no Dusk starting meanwhile finds it beside a record
        // that is no longer locked.
        remove_if_present(&self.autosave())?;
        let record = self.folder.join(format!("{}.{RECORD}", self.name));
        drop(self.record);
        remove_if_present(&record)
    }
}

/// The autosave of a Dusk that stopped without closing its project.
#[derive(Debug)]
pub struct Leftover {
    /// The project file it belongs to; `None` for an untitled project.
    pub project: Option<PathBuf>,
    /// The autosave.
    pub autosave: PathBuf,
    /// When it was written.
    pub saved: SystemTime,
    record: PathBuf,
}

impl Leftover {
    /// Removes its files.
    pub fn discard(self) -> io::Result<()> {
        remove_if_present(&self.autosave)?;
        remove_if_present(&self.record)
    }
}

/// The leftover autosaves in `folder`, newest first. Sessions that left no autosave, or
/// whose project file was saved after their autosave, are cleared away.
pub fn leftovers(folder: &Path) -> Vec<Leftover> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<Leftover> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension() == Some(OsStr::new(RECORD)))
        .filter_map(|record| leftover(&record))
        .collect();
    found.sort_by_key(|leftover| std::cmp::Reverse(leftover.saved));
    found
}

/// What the session of `record` left to recover, if that session is over. A finished
/// session with nothing worth recovering is cleared away.
fn leftover(record: &Path) -> Option<Leftover> {
    let mut file = File::options().read(true).write(true).open(record).ok()?;
    // The record of a running Dusk is locked.
    file.try_lock().ok()?;
    let mut text = Vec::new();
    file.read_to_end(&mut text).ok()?;
    drop(file);
    let text = String::from_utf8_lossy(&text);
    let project = (!text.is_empty()).then(|| PathBuf::from(text.as_ref()));
    let autosave = record.with_extension(AUTOSAVE);
    let modified = |path: &Path| std::fs::metadata(path).and_then(|meta| meta.modified());
    let saved = modified(&autosave).ok();
    let worth_keeping = saved.is_some_and(|saved| match &project {
        // A project file saved after the autosave holds everything the autosave does.
        Some(project) => !modified(project).is_ok_and(|written| written >= saved),
        None => true,
    });
    if !worth_keeping {
        let _ = remove_if_present(&autosave);
        let _ = remove_if_present(record);
        return None;
    }
    Some(Leftover {
        project,
        autosave,
        saved: saved?,
        record: record.to_path_buf(),
    })
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("dusk-recovery-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn files_in(dir: &Path) -> usize {
        std::fs::read_dir(dir).map_or(0, Iterator::count)
    }

    /// A session that autosaved `text` and then stopped without closing, as in a crash.
    fn crashed(dir: &Path, project: Option<&Path>, text: &str) -> PathBuf {
        let mut session = Session::start(dir).unwrap();
        session.record_project(project).unwrap();
        let autosave = session.autosave();
        std::fs::write(&autosave, text).unwrap();
        drop(session);
        autosave
    }

    #[test]
    fn a_running_session_is_not_a_leftover() {
        let dir = folder("running");
        let session = Session::start(&dir).unwrap();
        std::fs::write(session.autosave(), "{}").unwrap();
        assert!(leftovers(&dir).is_empty());
        // Looking did not disturb it.
        assert!(session.autosave().exists());
    }

    #[test]
    fn a_crashed_session_leaves_its_autosave_and_project() {
        let dir = folder("crashed");
        let project = dir.join("projects").join("edit.dusk");
        let autosave = crashed(&dir, Some(&project), "{\"version\": 1}");
        let found = leftovers(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].project.as_deref(), Some(project.as_path()));
        assert_eq!(found[0].autosave, autosave);
        assert_eq!(
            std::fs::read_to_string(&found[0].autosave).unwrap(),
            "{\"version\": 1}"
        );
        // Nothing changes until the user decides.
        assert_eq!(leftovers(&dir).len(), 1);
    }

    #[test]
    fn an_untitled_project_is_recovered_too() {
        let dir = folder("untitled");
        crashed(&dir, None, "{}");
        let found = leftovers(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].project, None);
    }

    #[test]
    fn a_discarded_leftover_is_gone() {
        let dir = folder("discarded");
        crashed(&dir, None, "{}");
        let leftover = leftovers(&dir).pop().unwrap();
        leftover.discard().unwrap();
        assert!(leftovers(&dir).is_empty());
        assert_eq!(files_in(&dir), 0);
    }

    #[test]
    fn a_closed_session_leaves_nothing() {
        let dir = folder("closed");
        let mut session = Session::start(&dir).unwrap();
        session
            .record_project(Some(&dir.join("edit.dusk")))
            .unwrap();
        std::fs::write(session.autosave(), "{}").unwrap();
        session.close().unwrap();
        assert!(leftovers(&dir).is_empty());
        assert_eq!(files_in(&dir), 0);
    }

    #[test]
    fn a_session_without_an_autosave_is_cleared_away() {
        let dir = folder("nothing to recover");
        drop(Session::start(&dir).unwrap());
        assert!(leftovers(&dir).is_empty());
        assert_eq!(files_in(&dir), 0);
    }

    #[test]
    fn an_autosave_older_than_its_saved_project_is_cleared_away() {
        let dir = folder("saved since");
        std::fs::create_dir_all(&dir).unwrap();
        let project = dir.join("edit.dusk");
        let autosave = crashed(&dir, Some(&project), "{}");
        std::fs::write(&project, "saved later").unwrap();
        let later = SystemTime::now() + Duration::from_secs(60);
        File::options()
            .write(true)
            .open(&project)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(leftovers(&dir).is_empty());
        assert!(!autosave.exists());
        // The project file itself is the user's and stays.
        assert!(project.exists());
    }

    #[test]
    fn leftovers_come_newest_first() {
        let dir = folder("two");
        let older = crashed(&dir, None, "older");
        let newer = crashed(&dir, None, "newer");
        let past = SystemTime::now() - Duration::from_secs(600);
        File::options()
            .write(true)
            .open(&older)
            .unwrap()
            .set_modified(past)
            .unwrap();
        let found = leftovers(&dir);
        let order: Vec<&Path> = found.iter().map(|l| l.autosave.as_path()).collect();
        assert_eq!(order, [newer.as_path(), older.as_path()]);
    }

    #[test]
    fn two_sessions_never_share_files() {
        let dir = folder("apart");
        let (a, b) = (Session::start(&dir).unwrap(), Session::start(&dir).unwrap());
        assert_ne!(a.autosave(), b.autosave());
    }
}
