//! Where Dusk writes files.

use std::path::{Path, PathBuf};

/// Where an export of the project made from `source` goes, until M4 brings the export dialog:
/// beside the source as "<name> export.mp4", numbered so no existing file is replaced.
pub fn export_path(source: &Path) -> PathBuf {
    let folder = source.parent().unwrap_or(Path::new("."));
    let stem = source
        .file_stem()
        .map_or_else(|| "Dusk".into(), |stem| stem.to_string_lossy());
    let taken = |path: &Path| {
        let mut part = path.as_os_str().to_owned();
        part.push(".part");
        path.exists() || Path::new(&part).exists()
    };
    (1..)
        .map(|n| match n {
            1 => folder.join(format!("{stem} export.mp4")),
            n => folder.join(format!("{stem} export {n}.mp4")),
        })
        .find(|path| !taken(path))
        .unwrap_or_else(|| folder.join(format!("{stem} export.mp4")))
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
        assert_eq!(export_path(&source), dir.join("beach day export.mp4"));
    }

    #[test]
    fn existing_files_are_never_replaced() {
        let dir = folder("numbered");
        let source = dir.join("clip.mp4");
        std::fs::write(dir.join("clip export.mp4"), b"earlier export").unwrap();
        // A leftover from an export that was cut off counts as taken too.
        std::fs::write(dir.join("clip export 2.mp4.part"), b"").unwrap();
        assert_eq!(export_path(&source), dir.join("clip export 3.mp4"));
    }
}
