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

/// Whether `text`, what a string holds, has three spaces or more between two of its words.
fn has_gap(text: &str) -> bool {
    text.trim().contains("   ")
}

/// Whether a raw string (`r"`, `r#"`, `br"`) starts at `at`, and with how many `#`.
fn raw_string_at(chars: &[char], at: usize) -> Option<usize> {
    let starts_word =
        |at: usize| at == 0 || !(chars[at - 1].is_alphanumeric() || chars[at - 1] == '_');
    let r = match chars[at] {
        'r' if starts_word(at) => at,
        'b' if starts_word(at) && chars.get(at + 1) == Some(&'r') => at + 1,
        _ => return None,
    };
    let hashes = chars[r + 1..].iter().take_while(|&&c| c == '#').count();
    (chars.get(r + 1 + hashes) == Some(&'"')).then_some(hashes)
}

/// The lines of `source` (counted from 1) on which a string with a gap of spaces starts.
/// Strings are followed across lines as Rust reads them: a `\` at a line's end drops the line
/// break and the next line's indent, while a line break without one keeps both. Comments and
/// raw strings (shader sources, file templates) are not messages.
fn gaps(source: &str) -> Vec<usize> {
    let chars: Vec<char> = source.replace("\r\n", "\n").chars().collect();
    let (mut found, mut line, mut at) = (Vec::new(), 1, 0);
    let next = |at: usize| chars.get(at + 1).copied();
    while at < chars.len() {
        if let Some(hashes) = raw_string_at(&chars, at) {
            // Past the opening quote, then up to a quote followed by as many `#`.
            at += chars[at..].iter().position(|&c| c == '"').unwrap_or(0) + 1;
            while at < chars.len()
                && !(chars[at] == '"'
                    && chars[at + 1..].iter().take_while(|&&c| c == '#').count() >= hashes)
            {
                line += usize::from(chars[at] == '\n');
                at += 1;
            }
            at += 1 + hashes;
            continue;
        }
        match chars[at] {
            '\n' => {
                line += 1;
                at += 1;
            }
            '/' if next(at) == Some('/') => {
                while at < chars.len() && chars[at] != '\n' {
                    at += 1;
                }
            }
            '/' if next(at) == Some('*') => {
                at += 2;
                while at < chars.len() && !(chars[at] == '*' && next(at) == Some('/')) {
                    line += usize::from(chars[at] == '\n');
                    at += 1;
                }
                at += 2;
            }
            // A character such as '"' or '\'', or else a lifetime.
            '\'' if next(at) == Some('\\') => {
                at += 3;
                while at < chars.len() && chars[at] != '\'' {
                    at += 1;
                }
                at += 1;
            }
            '\'' if chars.get(at + 2) == Some(&'\'') => at += 3,
            '"' => {
                let start = line;
                let mut text = String::new();
                at += 1;
                while at < chars.len() && chars[at] != '"' {
                    match (chars[at], next(at)) {
                        ('\\', Some('\n')) => {
                            at += 1;
                            while at < chars.len() && chars[at].is_whitespace() {
                                line += usize::from(chars[at] == '\n');
                                at += 1;
                            }
                        }
                        // An escape, `\"` among them, stands for something that is not a space.
                        ('\\', _) => {
                            text.push('\\');
                            at += 2;
                        }
                        (c, _) => {
                            line += usize::from(c == '\n');
                            text.push(c);
                            at += 1;
                        }
                    }
                }
                at += 1;
                if has_gap(&text) {
                    found.push(start);
                }
            }
            _ => at += 1,
        }
    }
    found
}

#[test]
fn messages_broken_with_a_backslash_comments_and_raw_strings_have_no_gaps() {
    let source = "// \"a   comment\"\n\
                  fn f(c: char) -> &'static str {\n\
                  \x20   if c == '\"' || c == '\\'' { return r#\"   raw \"   \"#; }\n\
                  \x20   \"Pick the file the project used, or a copy \\\n\
                  \x20        of it.\"\n\
                  }\n";
    assert_eq!(gaps(source), Vec::<usize>::new());
}

#[test]
fn a_message_broken_over_lines_without_a_backslash_is_found() {
    let source =
        "fn f() {\n    say(\"Pick the file the project used, or a copy\n         of it.\");\n}\n";
    assert_eq!(gaps(source), [2]);
}

/// Each source file's code before its tests, which come last: their fixtures (an encoder
/// listing, a shortcuts file with spaces around its keys) are not messages.
fn sources() -> Vec<(PathBuf, Vec<String>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut sources = Vec::new();
    for name in CRATES {
        for file in rust_files(&root.join(name).join("src")) {
            let text = std::fs::read_to_string(&file).expect("read a source file");
            let code = text
                .lines()
                .take_while(|line| line.trim() != "#[cfg(test)]")
                .map(str::to_owned)
                .collect();
            sources.push((file, code));
        }
    }
    sources
}

#[test]
fn the_scanner_keeps_in_step_with_every_source_to_its_end() {
    // A gap after a file's last line is found only if the scanner is not still inside a
    // string or a comment it took for one.
    for (file, code) in sources() {
        let source = format!("{}\nconst END: &str = \"a   b\";\n", code.join("\n"));
        assert_eq!(
            gaps(&source).last(),
            Some(&(code.len() + 1)),
            "{}",
            file.display()
        );
    }
}

#[test]
fn no_message_has_a_gap_of_spaces() {
    let mut found = Vec::new();
    for (file, code) in sources() {
        for number in gaps(&code.join("\n")) {
            found.push(format!(
                "{}:{}: {}",
                file.display(),
                number,
                code[number - 1].trim()
            ));
        }
    }
    assert!(
        found.is_empty(),
        "gaps of spaces in strings:\n{}",
        found.join("\n")
    );
}
