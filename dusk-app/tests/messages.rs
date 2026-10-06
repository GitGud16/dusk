//! Messages are sentences. A message broken over lines in the source needs a `\` at each
//! line's end, which also drops the next line's indent; without it the message shows a gap of
//! spaces mid-sentence, as two did until M6. This reads every crate's sources for such gaps.

use std::path::{Path, PathBuf};

const CRATES: [&str; 7] = [
    "dusk-core",
    "dusk-media",
    "dusk-render",
    "dusk-audio",
    "dusk-engine",
    "dusk-app",
    "dusk-cli",
];

/// The `.rs` files under `dir`.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    files
}

/// Whether `text`, the inside of a string on one line, has three spaces or more between two
/// of its words.
fn has_gap(text: &str) -> bool {
    text.trim().contains("   ")
}

#[test]
fn no_message_has_a_gap_of_spaces() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut gaps = Vec::new();
    for name in CRATES {
        for file in rust_files(&root.join(name).join("src")) {
            let text = std::fs::read_to_string(&file).expect("read a source file");
            // A file's tests come last, and their fixtures (an encoder listing, a shortcuts
            // file with spaces around its keys) are not messages.
            let code = text
                .lines()
                .take_while(|line| line.trim() != "#[cfg(test)]");
            for (number, line) in code.enumerate() {
                // What lies between quotes: every second part, counting from the second.
                if line.split('"').skip(1).step_by(2).any(has_gap) {
                    gaps.push(format!(
                        "{}:{}: {}",
                        file.display(),
                        number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        gaps.is_empty(),
        "gaps of spaces in strings:\n{}",
        gaps.join("\n")
    );
}
