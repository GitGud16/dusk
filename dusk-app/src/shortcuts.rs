//! The central shortcut table (CLAUDE.md, "UI conventions"): every user action and its key,
//! so the shortcut list stays complete. Remapping arrives in M5.

use slint::platform::Key;

/// Something the user can do from the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    PlayPause,
    PlayBackward,
    Pause,
    PlayForward,
    StepBack,
    StepForward,
    GoToStart,
    GoToEnd,
    Undo,
    Redo,
    Export,
    CancelExport,
}

/// A key as Slint reports it: a character (lowercase for letters) or a named key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyName {
    Char(char),
    Named(Key),
}

/// A key with its modifiers, and what it does.
#[derive(Clone, Copy, Debug)]
pub struct Shortcut {
    pub key: KeyName,
    pub ctrl: bool,
    pub shift: bool,
    pub action: Action,
    /// What it does, for the shortcut list.
    pub description: &'static str,
}

const fn plain(key: KeyName, action: Action, description: &'static str) -> Shortcut {
    Shortcut {
        key,
        ctrl: false,
        shift: false,
        action,
        description,
    }
}

const fn ctrl(key: char, shift: bool, action: Action, description: &'static str) -> Shortcut {
    Shortcut {
        key: KeyName::Char(key),
        ctrl: true,
        shift,
        action,
        description,
    }
}

/// Every shortcut.
pub const SHORTCUTS: &[Shortcut] = &[
    plain(KeyName::Char(' '), Action::PlayPause, "Play or pause"),
    plain(KeyName::Char('j'), Action::PlayBackward, "Play backwards"),
    plain(KeyName::Char('k'), Action::Pause, "Pause"),
    plain(
        KeyName::Char('l'),
        Action::PlayForward,
        "Play; press again to play faster",
    ),
    plain(
        KeyName::Named(Key::LeftArrow),
        Action::StepBack,
        "Previous frame",
    ),
    plain(
        KeyName::Named(Key::RightArrow),
        Action::StepForward,
        "Next frame",
    ),
    plain(
        KeyName::Named(Key::Home),
        Action::GoToStart,
        "Go to the start",
    ),
    plain(
        KeyName::Named(Key::End),
        Action::GoToEnd,
        "Go to the last frame",
    ),
    ctrl('z', false, Action::Undo, "Undo"),
    ctrl('z', true, Action::Redo, "Redo"),
    ctrl('y', false, Action::Redo, "Redo"),
    ctrl('e', false, Action::Export, "Export to MP4"),
    plain(
        KeyName::Named(Key::Escape),
        Action::CancelExport,
        "Cancel the export",
    ),
];

impl Shortcut {
    /// How the shortcut list writes the keys, such as "Ctrl+Shift+Z".
    pub fn keys(&self) -> String {
        let key = match self.key {
            KeyName::Char(' ') => "Space".to_owned(),
            KeyName::Char(c) => c.to_ascii_uppercase().to_string(),
            KeyName::Named(Key::LeftArrow) => "Left".to_owned(),
            KeyName::Named(Key::RightArrow) => "Right".to_owned(),
            KeyName::Named(Key::Escape) => "Esc".to_owned(),
            KeyName::Named(named) => format!("{named:?}"),
        };
        let ctrl = if self.ctrl { "Ctrl+" } else { "" };
        let shift = if self.shift { "Shift+" } else { "" };
        format!("{ctrl}{shift}{key}")
    }
}

/// The action for a key press; `text` is the key text Slint reports.
pub fn action_for(text: &str, ctrl: bool, shift: bool, alt: bool) -> Option<Action> {
    let mut chars = text.chars();
    let (Some(key), None) = (chars.next(), chars.next()) else {
        return None;
    };
    let key = key.to_ascii_lowercase();
    if alt {
        return None;
    }
    SHORTCUTS
        .iter()
        .find(|shortcut| {
            let matches_key = match shortcut.key {
                KeyName::Char(c) => c == key,
                KeyName::Named(named) => char::from(named) == key,
            };
            matches_key && shortcut.ctrl == ctrl && shortcut.shift == shift
        })
        .map(|shortcut| shortcut.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(key: Key) -> String {
        slint::SharedString::from(key).to_string()
    }

    #[test]
    fn plain_keys_find_their_action() {
        assert_eq!(
            action_for(" ", false, false, false),
            Some(Action::PlayPause)
        );
        assert_eq!(
            action_for("l", false, false, false),
            Some(Action::PlayForward)
        );
        assert_eq!(
            action_for(&named(Key::LeftArrow), false, false, false),
            Some(Action::StepBack)
        );
        assert_eq!(
            action_for(&named(Key::End), false, false, false),
            Some(Action::GoToEnd)
        );
    }

    #[test]
    fn letters_match_in_either_case() {
        assert_eq!(
            action_for("L", false, false, false),
            Some(Action::PlayForward)
        );
        assert_eq!(action_for("Z", true, true, false), Some(Action::Redo));
    }

    #[test]
    fn modifiers_must_match() {
        assert_eq!(action_for("z", true, false, false), Some(Action::Undo));
        assert_eq!(action_for("z", true, true, false), Some(Action::Redo));
        assert_eq!(action_for("z", false, false, false), None);
        assert_eq!(action_for("l", true, false, false), None);
        assert_eq!(action_for("l", false, false, true), None);
        assert_eq!(action_for("x", false, false, false), None);
    }

    #[test]
    fn the_list_names_keys_as_printed_on_the_keyboard() {
        let keys: Vec<String> = SHORTCUTS.iter().map(Shortcut::keys).collect();
        for expected in [
            "Space",
            "J",
            "Left",
            "Home",
            "End",
            "Ctrl+Z",
            "Ctrl+Shift+Z",
            "Esc",
        ] {
            assert!(keys.iter().any(|k| k == expected), "{expected} in {keys:?}");
        }
    }

    #[test]
    fn no_two_shortcuts_share_a_key() {
        for (i, a) in SHORTCUTS.iter().enumerate() {
            for b in &SHORTCUTS[i + 1..] {
                assert!(
                    (a.key, a.ctrl, a.shift) != (b.key, b.ctrl, b.shift),
                    "{a:?} and {b:?}"
                );
            }
        }
    }
}
