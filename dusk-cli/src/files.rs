//! Where dusq writes: a name beside the input that replaces nothing, unless told otherwise,
//! and never over the input itself.

use std::path::{Path, PathBuf};

/// The file a job writes to when not told: `<name><suffix>.<extension>` beside `input`, or
/// with 2, 3 and so on after it, the first that replaces no file, not even the part file of
/// a job that was cut off.
pub fn beside(input: &Path, suffix: &str, extension: &str) -> PathBuf {
    let folder = input.parent().unwrap_or(Path::new("."));
    let stem = input
        .file_stem()
        .map_or_else(|| "output".into(), |stem| stem.to_string_lossy());
    let taken = |path: &Path| path.exists() || part(path).exists();
    (1..)
        .map(|n| match n {
            1 => folder.join(format!("{stem}{suffix}.{extension}")),
            n => folder.join(format!("{stem}{suffix} {n}.{extension}")),
        })
        .find(|path| !taken(path))
        .unwrap_or_else(|| folder.join(format!("{stem}{suffix}.{extension}")))
}

/// Whether `a` and `b` are the same file, however they are written.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        // One is not there, so they are not the same file.
        _ => false,
    }
}

/// `path` with `.part` appended, as the engine writes before renaming.
fn part(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dusq-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_name_beside_the_input_replaces_nothing() {
        let dir = folder("beside");
        let input = dir.join("beach day.mp4");
        std::fs::write(&input, b"video").unwrap();
        assert_eq!(
            beside(&input, " compressed", "mp4"),
            dir.join("beach day compressed.mp4")
        );
        std::fs::write(dir.join("beach day compressed.mp4"), b"earlier").unwrap();
        std::fs::write(dir.join("beach day compressed 2.mp4.part"), b"cut off").unwrap();
        assert_eq!(
            beside(&input, " compressed", "mp4"),
            dir.join("beach day compressed 3.mp4")
        );
        // Sound of the same format as the input steps around it.
        let song = dir.join("song.mp3");
        std::fs::write(&song, b"sound").unwrap();
        assert_eq!(beside(&song, "", "mp3"), dir.join("song 2.mp3"));
        assert_eq!(beside(&song, "", "wav"), dir.join("song.wav"));
    }

    #[test]
    fn a_file_is_the_same_however_it_is_written() {
        let dir = folder("same");
        let file = dir.join("clip.mp4");
        std::fs::write(&file, b"video").unwrap();
        assert!(same_file(&file, &dir.join(".").join("clip.mp4")));
        assert!(!same_file(&file, &dir.join("other.mp4")));
        // A file that is not there yet is not any other file.
        assert!(!same_file(&dir.join("new.mp4"), &file));
    }
}
