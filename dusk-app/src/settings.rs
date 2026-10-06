//! What Dusk keeps in its settings folder (docs/ARCHITECTURE.md, "Keyboard and settings"):
//! the user's shortcuts, read once when Dusk starts and written on the file worker.

use std::io;
use std::path::Path;

use crate::files::write_atomically;
use crate::keymap::Keymap;

/// The shortcuts file's name in the settings folder.
pub const SHORTCUTS_FILE: &str = "shortcuts.txt";

/// The keymap the shortcuts file in `dir` keeps, and what in it could not be read, each a
/// sentence for the status line; the defaults when there is no folder or no file.
pub fn read_keymap(dir: Option<&Path>) -> (Keymap, Vec<String>) {
    let Some(dir) = dir else {
        return (Keymap::default(), Vec::new());
    };
    let path = dir.join(SHORTCUTS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let (map, problems) = Keymap::read(&text);
            let problems = problems
                .into_iter()
                .map(|problem| format!("{SHORTCUTS_FILE}, {problem}; that line is skipped."))
                .collect();
            (map, problems)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (Keymap::default(), Vec::new())
        }
        Err(error) => (
            Keymap::default(),
            vec![format!(
                "Dusk could not read {}: {error}; the default keys are used.",
                path.display()
            )],
        ),
    }
}

/// Writes the shortcuts file that keeps `keymap` into `dir`, which it makes if needed: as
/// "<name>.part" first, renamed into place once flushed. It does file work, so it runs on the
/// file worker.
pub fn write_keymap(dir: &Path, keymap: &Keymap) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    write_atomically(&dir.join(SHORTCUTS_FILE), keymap.to_file().as_bytes())
}

/// What the status line says of `problems`: the first, and how many more there are.
pub fn problems_message(problems: &[String]) -> Option<String> {
    let (first, rest) = problems.split_first()?;
    Some(match rest.len() {
        0 => first.clone(),
        1 => format!("{first} 1 more line was skipped."),
        more => format!("{first} {more} more lines were skipped."),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcuts::Action;

    /// A folder of its own for one test.
    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dusk-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn without_a_file_the_keys_are_the_defaults() {
        let dir = folder("none");
        assert_eq!(read_keymap(Some(&dir)), (Keymap::default(), Vec::new()));
        assert_eq!(read_keymap(None), (Keymap::default(), Vec::new()));
    }

    #[test]
    fn the_file_changes_the_keys_and_says_what_it_skipped() {
        let dir = folder("some");
        std::fs::write(dir.join(SHORTCUTS_FILE), "split = X\nsplitt = Y\n").unwrap();
        let (map, problems) = read_keymap(Some(&dir));
        assert_eq!(map.keys_of(Action::Split).len(), 1);
        assert_eq!(map.keys_of(Action::Split)[0].to_string(), "X");
        assert_eq!(
            problems,
            ["shortcuts.txt, line 2: Dusk has no action called \"splitt\"; that line is skipped."]
        );
    }

    #[test]
    fn a_file_that_cannot_be_read_leaves_the_defaults() {
        let dir = folder("broken");
        std::fs::write(dir.join(SHORTCUTS_FILE), [0xFF, 0xFE, 0x00, 0x41]).unwrap();
        let (map, problems) = read_keymap(Some(&dir));
        assert_eq!(map, Keymap::default());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("shortcuts.txt"), "{}", problems[0]);
        assert!(
            problems[0].ends_with("the default keys are used."),
            "{}",
            problems[0]
        );
    }

    #[test]
    fn a_written_keymap_reads_back_the_same() {
        let dir = folder("written").join("Dusk");
        let (changed, _) = Keymap::read(
            "split = X
unlink =
",
        );
        write_keymap(&dir, &changed).unwrap();
        assert_eq!(read_keymap(Some(&dir)), (changed, Vec::new()));
        // The defaults make a file of comments that names every action.
        write_keymap(&dir, &Keymap::default()).unwrap();
        let text = std::fs::read_to_string(dir.join(SHORTCUTS_FILE)).unwrap();
        assert!(text.contains("# split = S"), "{text}");
        assert_eq!(read_keymap(Some(&dir)), (Keymap::default(), Vec::new()));
    }

    #[test]
    fn the_status_line_names_the_first_problem_and_counts_the_rest() {
        assert_eq!(problems_message(&[]), None);
        let one = vec!["shortcuts.txt, line 2: X; that line is skipped.".to_owned()];
        assert_eq!(problems_message(&one).as_deref(), Some(one[0].as_str()));
        let three = vec![one[0].clone(), "b".to_owned(), "c".to_owned()];
        assert_eq!(
            problems_message(&three).as_deref(),
            Some("shortcuts.txt, line 2: X; that line is skipped. 2 more lines were skipped.")
        );
    }
}
