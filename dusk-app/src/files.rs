//! Where Dusk writes files, and how: never half-written where the user expects them.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;

/// A piece of file work.
type Job = Box<dyn FnOnce() + Send>;

/// Runs file work on a thread of its own, one job at a time in the order given, so the UI
/// thread never waits for a disk and writes never overtake each other. Dropping it waits for
/// the jobs already given, so a save started before quitting still finishes.
pub struct Worker {
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Starts the worker's thread.
    pub fn start() -> io::Result<Worker> {
        let (jobs, inbox) = channel::<Job>();
        let thread = std::thread::Builder::new()
            .name("dusk files".to_owned())
            .spawn(move || {
                for job in inbox {
                    job();
                }
            })?;
        Ok(Worker {
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }

    /// Queues `job`.
    pub fn run(&self, job: impl FnOnce() + Send + 'static) {
        if let Some(jobs) = &self.jobs {
            // The thread only stops once the sender is gone, so this cannot fail.
            let _ = jobs.send(Box::new(job));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Without a sender the thread finishes the queued jobs and ends.
        drop(self.jobs.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Writes `contents` to `path` without ever leaving a half-written file there: into
/// "<path>.part" first, flushed to disk, then renamed over `path`. On failure the part file
/// is removed and `path` is left as it was.
pub fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let part = part_path(path);
    let written = (|| {
        let mut file = std::fs::File::create(&part)?;
        io::Write::write_all(&mut file, contents)?;
        // On disk before the rename, so a crash cannot leave an empty file at `path`.
        file.sync_all()?;
        drop(file);
        std::fs::rename(&part, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written
}

/// Where a file is written before it is renamed to `path`.
pub fn part_path(path: &Path) -> PathBuf {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    PathBuf::from(part)
}

/// The name the clip editor suggests for an export of a clip of `source`: "<name> edit.mp4"
/// in `folder`, numbered so no existing file is replaced.
pub fn clip_export_path(folder: &Path, source: &Path, extension: &str) -> PathBuf {
    free_file(folder, &format!("{} edit", stem(source)), extension)
}

/// Whether `a` and `b` name the same existing file, however their paths are written.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// `name`.`extension` in `folder`, or `name` 2, 3 and so on: the first that replaces no
/// file, not even the part file of an export that was cut off.
fn free_file(folder: &Path, name: &str, extension: &str) -> PathBuf {
    let taken = |path: &Path| path.exists() || part_path(path).exists();
    (1..)
        .map(|n| match n {
            1 => folder.join(format!("{name}.{extension}")),
            n => folder.join(format!("{name} {n}.{extension}")),
        })
        .find(|path| !taken(path))
        .unwrap_or_else(|| folder.join(format!("{name}.{extension}")))
}

/// The file name of `path` without its extension; "Dusk" when it has none.
fn stem(path: &Path) -> std::borrow::Cow<'_, str> {
    path.file_stem()
        .map_or_else(|| "Dusk".into(), |stem| stem.to_string_lossy())
}

/// The name the export dialog suggests for an export of the project made from `source`:
/// beside it as "<name> export.<extension>", numbered so no existing file is replaced.
pub fn export_path(source: &Path, extension: &str) -> PathBuf {
    let folder = source.parent().unwrap_or(Path::new("."));
    free_file(folder, &format!("{} export", stem(source)), extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("dusk-files-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_export_goes_beside_the_source() {
        let dir = folder("beside");
        let source = dir.join("beach day.mov");
        assert_eq!(
            export_path(&source, "mp4"),
            dir.join("beach day export.mp4")
        );
        assert_eq!(
            export_path(&source, "mkv"),
            dir.join("beach day export.mkv")
        );
    }

    #[test]
    fn jobs_run_in_order_and_finish_before_the_worker_goes() {
        let done = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let worker = Worker::start().unwrap();
        for n in 0..20 {
            let done = std::sync::Arc::clone(&done);
            worker.run(move || {
                std::thread::sleep(std::time::Duration::from_millis(1));
                done.lock().unwrap().push(n);
            });
        }
        drop(worker);
        assert_eq!(*done.lock().unwrap(), (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn a_file_is_written_whole_with_nothing_left_beside_it() {
        let dir = folder("whole");
        let path = dir.join("edit.dusk");
        write_atomically(&path, b"first").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        write_atomically(&path, b"second, longer").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second, longer");
        assert!(!part_path(&path).exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn a_failed_write_leaves_things_as_they_were() {
        let dir = folder("failed");
        // A folder where the file should go cannot be replaced by it.
        let path = dir.join("edit.dusk");
        std::fs::create_dir(&path).unwrap();
        assert!(write_atomically(&path, b"text").is_err());
        assert!(path.is_dir());
        assert!(!part_path(&path).exists());
        // Nor can a file be written into a folder that does not exist.
        let nowhere = dir.join("missing").join("edit.dusk");
        assert!(write_atomically(&nowhere, b"text").is_err());
        assert!(!nowhere.exists() && !part_path(&nowhere).exists());
    }

    #[test]
    fn a_clip_export_is_named_after_its_source() {
        let dir = folder("clip-export");
        let source = Path::new("D:/phone/IMG_0042.MOV");
        assert_eq!(
            clip_export_path(&dir, source, "mp4"),
            dir.join("IMG_0042 edit.mp4")
        );
        std::fs::write(dir.join("IMG_0042 edit.mp4"), b"earlier").unwrap();
        assert_eq!(
            clip_export_path(&dir, source, "mp4"),
            dir.join("IMG_0042 edit 2.mp4")
        );
        assert_eq!(
            clip_export_path(&dir, source, "opus"),
            dir.join("IMG_0042 edit.opus")
        );
    }

    #[test]
    fn a_file_is_known_however_its_path_is_written() {
        let dir = folder("same");
        let media = dir.join("clip.mp4");
        std::fs::write(&media, b"media").unwrap();
        assert!(same_file(&media, &dir.join(".").join("clip.mp4")));
        #[cfg(windows)]
        assert!(same_file(&media, &dir.join("CLIP.MP4")));
        // A file that is not there is no file of the project's.
        assert!(!same_file(&media, &dir.join("clip edit.mp4")));
    }

    #[test]
    fn existing_files_are_never_replaced() {
        let dir = folder("numbered");
        let source = dir.join("clip.mp4");
        std::fs::write(dir.join("clip export.mp4"), b"earlier export").unwrap();
        // A leftover from an export that was cut off counts as taken too.
        std::fs::write(dir.join("clip export 2.mp4.part"), b"").unwrap();
        assert_eq!(export_path(&source, "mp4"), dir.join("clip export 3.mp4"));
    }
}
