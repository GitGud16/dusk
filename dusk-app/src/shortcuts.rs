//! The central shortcut table (CLAUDE.md, "UI conventions"): every user action and its key,
//! so the shortcut list stays complete. Menu items name the same actions. Remapping arrives
//! in M5.

use slint::platform::Key;

/// Something the user can do.
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
    Split,
    Delete,
    RippleDelete,
    DeleteOne,
    Unlink,
    ToggleEnabled,
    Place,
    /// Hides or shows, mutes or unmutes, the track at this index (V1, V2, A1, A2).
    ToggleMute(usize),
    /// Locks or unlocks the track at this index.
    ToggleLock(usize),
    ZoomIn,
    ZoomOut,
    ZoomFit,
    NewProject,
    OpenProject,
    Save,
    SaveAs,
    Import,
    SequenceSettings,
    Export,
    CancelExport,
    Quit,
    ShortcutList,
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
    pub alt: bool,
    pub action: Action,
    /// What it does, for the shortcut list.
    pub description: &'static str,
}

const fn key(key: KeyName, action: Action, description: &'static str) -> Shortcut {
    Shortcut {
        key,
        ctrl: false,
        shift: false,
        alt: false,
        action,
        description,
    }
}

const fn char_key(c: char, action: Action, description: &'static str) -> Shortcut {
    key(KeyName::Char(c), action, description)
}

const fn ctrl(shortcut: Shortcut) -> Shortcut {
    Shortcut {
        ctrl: true,
        ..shortcut
    }
}

const fn shift(shortcut: Shortcut) -> Shortcut {
    Shortcut {
        shift: true,
        ..shortcut
    }
}

const fn alt(shortcut: Shortcut) -> Shortcut {
    Shortcut {
        alt: true,
        ..shortcut
    }
}

/// Every shortcut.
pub const SHORTCUTS: &[Shortcut] = &[
    char_key(' ', Action::PlayPause, "Play or pause"),
    char_key('j', Action::PlayBackward, "Play backwards"),
    char_key('k', Action::Pause, "Pause"),
    char_key('l', Action::PlayForward, "Play; press again to play faster"),
    key(
        KeyName::Named(Key::LeftArrow),
        Action::StepBack,
        "Previous frame",
    ),
    key(
        KeyName::Named(Key::RightArrow),
        Action::StepForward,
        "Next frame",
    ),
    key(
        KeyName::Named(Key::Home),
        Action::GoToStart,
        "Go to the start",
    ),
    key(
        KeyName::Named(Key::End),
        Action::GoToEnd,
        "Go to the last frame",
    ),
    ctrl(char_key('z', Action::Undo, "Undo")),
    ctrl(shift(char_key('z', Action::Redo, "Redo"))),
    ctrl(char_key('y', Action::Redo, "Redo")),
    char_key(
        's',
        Action::Split,
        "Split at the playhead: the selected clip, or every clip there",
    ),
    key(
        KeyName::Named(Key::Delete),
        Action::Delete,
        "Delete the selected clip and its linked clips, leaving a gap",
    ),
    shift(key(
        KeyName::Named(Key::Delete),
        Action::RippleDelete,
        "Delete and close the gap on every unlocked track",
    )),
    alt(key(
        KeyName::Named(Key::Delete),
        Action::DeleteOne,
        "Delete the selected clip only, not its linked clips",
    )),
    ctrl(char_key(
        'l',
        Action::Unlink,
        "Detach audio: unlink the selected clip",
    )),
    char_key(
        'e',
        Action::ToggleEnabled,
        "Enable or disable the selected clip",
    ),
    char_key(
        'p',
        Action::Place,
        "Place the selected media at the playhead",
    ),
    alt(char_key('1', Action::ToggleMute(0), "Hide or show V1")),
    alt(char_key('2', Action::ToggleMute(1), "Hide or show V2")),
    alt(char_key('3', Action::ToggleMute(2), "Mute or unmute A1")),
    alt(char_key('4', Action::ToggleMute(3), "Mute or unmute A2")),
    ctrl(alt(char_key(
        '1',
        Action::ToggleLock(0),
        "Lock or unlock V1",
    ))),
    ctrl(alt(char_key(
        '2',
        Action::ToggleLock(1),
        "Lock or unlock V2",
    ))),
    ctrl(alt(char_key(
        '3',
        Action::ToggleLock(2),
        "Lock or unlock A1",
    ))),
    ctrl(alt(char_key(
        '4',
        Action::ToggleLock(3),
        "Lock or unlock A2",
    ))),
    char_key('=', Action::ZoomIn, "Zoom in"),
    char_key('-', Action::ZoomOut, "Zoom out"),
    char_key('\\', Action::ZoomFit, "Fit the sequence in view"),
    ctrl(char_key('n', Action::NewProject, "New project")),
    ctrl(char_key('o', Action::OpenProject, "Open a project")),
    ctrl(char_key('s', Action::Save, "Save the project")),
    ctrl(shift(char_key(
        's',
        Action::SaveAs,
        "Save the project under a new name",
    ))),
    ctrl(char_key('i', Action::Import, "Import media")),
    ctrl(shift(char_key(
        'r',
        Action::SequenceSettings,
        "Sequence settings: frame rate and size",
    ))),
    ctrl(char_key('e', Action::Export, "Export to MP4")),
    key(
        KeyName::Named(Key::Escape),
        Action::CancelExport,
        "Cancel the export",
    ),
    ctrl(char_key('q', Action::Quit, "Quit")),
    char_key('?', Action::ShortcutList, "Show the keyboard shortcuts"),
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
        let alt = if self.alt { "Alt+" } else { "" };
        let shift = if self.shift { "Shift+" } else { "" };
        format!("{ctrl}{alt}{shift}{key}")
    }

    /// Whether the key ignores Shift: a punctuation character already says whether Shift
    /// was held, and which keys need it differs between keyboard layouts.
    fn shift_agnostic(&self) -> bool {
        matches!(self.key, KeyName::Char(c) if c.is_ascii_punctuation())
    }
}

/// The action for a key press; `text` is the key text Slint reports.
pub fn action_for(text: &str, ctrl: bool, shift: bool, alt: bool) -> Option<Action> {
    let mut chars = text.chars();
    let (Some(key), None) = (chars.next(), chars.next()) else {
        return None;
    };
    let key = key.to_ascii_lowercase();
    SHORTCUTS
        .iter()
        .find(|shortcut| {
            let matches_key = match shortcut.key {
                KeyName::Char(c) => c == key,
                KeyName::Named(named) => char::from(named) == key,
            };
            matches_key
                && shortcut.ctrl == ctrl
                && shortcut.alt == alt
                && (shortcut.shift == shift || shortcut.shift_agnostic())
        })
        .map(|shortcut| shortcut.action)
}

/// The action a menu item or button names (see `root.action(...)` in ui/app.slint).
pub fn action_named(name: &str) -> Option<Action> {
    Some(match name {
        "new" => Action::NewProject,
        "open" => Action::OpenProject,
        "save" => Action::Save,
        "save-as" => Action::SaveAs,
        "import" => Action::Import,
        "export" => Action::Export,
        "cancel-export" => Action::CancelExport,
        "quit" => Action::Quit,
        "undo" => Action::Undo,
        "redo" => Action::Redo,
        "split" => Action::Split,
        "delete" => Action::Delete,
        "ripple-delete" => Action::RippleDelete,
        "delete-one" => Action::DeleteOne,
        "unlink" => Action::Unlink,
        "toggle-enabled" => Action::ToggleEnabled,
        "place" => Action::Place,
        "sequence-settings" => Action::SequenceSettings,
        "zoom-in" => Action::ZoomIn,
        "zoom-out" => Action::ZoomOut,
        "zoom-fit" => Action::ZoomFit,
        _ => return None,
    })
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
        assert_eq!(action_for("s", false, false, false), Some(Action::Split));
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
        assert_eq!(action_for("l", true, false, false), Some(Action::Unlink));
        assert_eq!(action_for("l", false, false, true), None);
        assert_eq!(action_for("x", false, false, false), None);
        assert_eq!(action_for("s", true, false, false), Some(Action::Save));
        assert_eq!(action_for("s", true, true, false), Some(Action::SaveAs));
    }

    #[test]
    fn delete_comes_in_three_kinds() {
        let delete = named(Key::Delete);
        assert_eq!(
            action_for(&delete, false, false, false),
            Some(Action::Delete)
        );
        assert_eq!(
            action_for(&delete, false, true, false),
            Some(Action::RippleDelete)
        );
        assert_eq!(
            action_for(&delete, false, false, true),
            Some(Action::DeleteOne)
        );
    }

    #[test]
    fn tracks_are_hidden_with_alt_and_locked_with_ctrl_alt() {
        assert_eq!(
            action_for("3", false, false, true),
            Some(Action::ToggleMute(2))
        );
        assert_eq!(
            action_for("1", true, false, true),
            Some(Action::ToggleLock(0))
        );
        assert_eq!(action_for("1", false, false, false), None);
    }

    #[test]
    fn punctuation_keys_work_with_or_without_shift() {
        // "?" needs Shift on most layouts; which key it is varies.
        assert_eq!(
            action_for("?", false, true, false),
            Some(Action::ShortcutList)
        );
        assert_eq!(
            action_for("?", false, false, false),
            Some(Action::ShortcutList)
        );
        assert_eq!(action_for("=", false, false, false), Some(Action::ZoomIn));
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
            "Delete",
            "Shift+Delete",
            "Alt+Delete",
            "Ctrl+Alt+1",
            "?",
        ] {
            assert!(keys.iter().any(|k| k == expected), "{expected} in {keys:?}");
        }
    }

    #[test]
    fn no_two_shortcuts_share_a_key() {
        for (i, a) in SHORTCUTS.iter().enumerate() {
            for b in &SHORTCUTS[i + 1..] {
                assert!(
                    (a.key, a.ctrl, a.shift, a.alt) != (b.key, b.ctrl, b.shift, b.alt),
                    "{a:?} and {b:?}"
                );
            }
        }
    }

    #[test]
    fn every_action_the_window_names_exists() {
        let ui = include_str!("../ui/app.slint");
        let names: Vec<&str> = ui
            .split("root.action(\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect();
        assert!(names.len() > 15, "{names:?}");
        for name in names {
            assert!(action_named(name).is_some(), "{name}");
        }
    }

    #[test]
    fn every_named_action_has_a_shortcut() {
        let ui = include_str!("../ui/app.slint");
        for name in ui.split("root.action(\"").skip(1) {
            let action = name.split('"').next().and_then(action_named).unwrap();
            assert!(
                SHORTCUTS.iter().any(|shortcut| shortcut.action == action),
                "{action:?} has no shortcut"
            );
        }
    }
}
