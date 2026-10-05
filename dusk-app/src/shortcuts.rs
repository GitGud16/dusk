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
    PlaySlowBackward,
    PlaySlowForward,
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
    /// Fits the picture with bars or fills the frame: the selected clip's, or the clip
    /// editor's draft.
    ToggleFill,
    /// Opens the selected clip in the clip editor.
    OpenClipEditor,
    /// The clip editor's own: start or end the clip at the playhead, turn and mirror the
    /// picture, apply the draft to the project, close the window.
    MarkIn,
    MarkOut,
    TurnLeft,
    TurnRight,
    MirrorLeftRight,
    MirrorTopBottom,
    ApplyClip,
    CloseClipEditor,
    /// When the clip changed in the main window: the clip editor takes it as it is now,
    /// or keeps its draft to apply over it.
    ReloadClip,
    KeepDraft,
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
    char_key(
        'j',
        Action::PlayBackward,
        "Play backwards; press again to play faster, up to 32x",
    ),
    char_key('k', Action::Pause, "Pause"),
    char_key(
        'l',
        Action::PlayForward,
        "Play; press again to play faster, up to 32x",
    ),
    shift(char_key(
        'j',
        Action::PlaySlowBackward,
        "Play backwards slowly; press again to play slower, down to 0.1x",
    )),
    shift(char_key(
        'l',
        Action::PlaySlowForward,
        "Play slowly; press again to play slower, down to 0.1x",
    )),
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
    char_key(
        'f',
        Action::ToggleFill,
        "Fit the picture with bars, or fill the frame and crop",
    ),
    key(
        KeyName::Named(Key::Return),
        Action::OpenClipEditor,
        "Open the selected clip in the clip editor",
    ),
    char_key(
        'i',
        Action::MarkIn,
        "Clip editor: start the clip at the playhead",
    ),
    char_key(
        'o',
        Action::MarkOut,
        "Clip editor: end the clip at the playhead",
    ),
    char_key(
        'r',
        Action::TurnRight,
        "Clip editor: turn the picture right",
    ),
    shift(char_key(
        'r',
        Action::TurnLeft,
        "Clip editor: turn the picture left",
    )),
    char_key(
        'h',
        Action::MirrorLeftRight,
        "Clip editor: mirror the picture left to right",
    ),
    char_key(
        'v',
        Action::MirrorTopBottom,
        "Clip editor: mirror the picture top to bottom",
    ),
    ctrl(key(
        KeyName::Named(Key::Return),
        Action::ApplyClip,
        "Clip editor: apply the changes to the project",
    )),
    ctrl(char_key(
        'w',
        Action::CloseClipEditor,
        "Close the clip editor",
    )),
    ctrl(char_key(
        'r',
        Action::ReloadClip,
        "Clip editor: the clip changed in the main window; take it as it is now",
    )),
    ctrl(char_key(
        'k',
        Action::KeepDraft,
        "Clip editor: the clip changed in the main window; keep the draft",
    )),
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
            KeyName::Named(Key::Return) => "Enter".to_owned(),
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

/// The action for a key press: by `key`, the letter or digit of the key pressed when it has
/// one (see `platform::pressed_key`), otherwise by `text`. Letter shortcuts so work under any
/// keyboard layout, as Windows' own do: under Arabic (101), L types م, and Shift+L a slash.
pub fn action_for_key(
    text: &str,
    key: Option<char>,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> Option<Action> {
    match key {
        Some(key) => action_for(key.encode_utf8(&mut [0; 4]), ctrl, shift, alt),
        None => action_for(text, ctrl, shift, alt),
    }
}

/// The shortcut list as the windows show it.
pub fn shortcut_list() -> slint::ModelRc<crate::ShortcutView> {
    let list: Vec<crate::ShortcutView> = SHORTCUTS
        .iter()
        .map(|shortcut| crate::ShortcutView {
            keys: shortcut.keys().into(),
            description: shortcut.description.into(),
        })
        .collect();
    std::rc::Rc::new(slint::VecModel::from(list)).into()
}

/// The action a menu item or button names (see `root.action(...)` in the windows' .slint
/// files).
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
        "toggle-fill" => Action::ToggleFill,
        "open-clip-editor" => Action::OpenClipEditor,
        "apply-clip" => Action::ApplyClip,
        "reload-clip" => Action::ReloadClip,
        "keep-draft" => Action::KeepDraft,
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
    fn letter_keys_work_under_any_keyboard_layout() {
        // Arabic (101): L types م, and Shift+L a slash; the key is L either way.
        assert_eq!(
            action_for_key("م", Some('l'), false, false, false),
            Some(Action::PlayForward)
        );
        assert_eq!(
            action_for_key("/", Some('l'), false, true, false),
            Some(Action::PlaySlowForward)
        );
        assert_eq!(
            action_for_key("س", Some('s'), true, false, false),
            Some(Action::Save)
        );
        // Keys without a letter or digit go by their text.
        assert_eq!(
            action_for_key(" ", None, false, false, false),
            Some(Action::PlayPause)
        );
        assert_eq!(
            action_for_key(&named(Key::Delete), None, false, false, false),
            action_for(&named(Key::Delete), false, false, false)
        );
    }

    #[test]
    fn the_clip_editor_has_keys_of_its_own() {
        let enter = named(Key::Return);
        assert_eq!(
            action_for(&enter, false, false, false),
            Some(Action::OpenClipEditor)
        );
        assert_eq!(
            action_for(&enter, true, false, false),
            Some(Action::ApplyClip)
        );
        assert_eq!(action_for("i", false, false, false), Some(Action::MarkIn));
        assert_eq!(action_for("o", false, false, false), Some(Action::MarkOut));
        assert_eq!(
            action_for("r", false, false, false),
            Some(Action::TurnRight)
        );
        assert_eq!(action_for("R", false, true, false), Some(Action::TurnLeft));
        assert_eq!(
            action_for("h", false, false, false),
            Some(Action::MirrorLeftRight)
        );
        assert_eq!(
            action_for("v", false, false, false),
            Some(Action::MirrorTopBottom)
        );
        assert_eq!(
            action_for("f", false, false, false),
            Some(Action::ToggleFill)
        );
        assert_eq!(
            action_for("w", true, false, false),
            Some(Action::CloseClipEditor)
        );
        assert_eq!(
            action_for("r", true, false, false),
            Some(Action::ReloadClip)
        );
        assert_eq!(action_for("k", true, false, false), Some(Action::KeepDraft));
    }

    #[test]
    fn shift_plays_slowly() {
        assert_eq!(
            action_for("L", false, true, false),
            Some(Action::PlaySlowForward)
        );
        assert_eq!(
            action_for("J", false, true, false),
            Some(Action::PlaySlowBackward)
        );
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

    /// The actions the windows' menu items and buttons name.
    fn named_in_the_windows() -> Vec<&'static str> {
        [
            include_str!("../ui/app.slint"),
            include_str!("../ui/clip-editor.slint"),
        ]
        .into_iter()
        .flat_map(|ui| ui.split("root.action(\"").skip(1))
        .filter_map(|rest| rest.split('"').next())
        .collect()
    }

    #[test]
    fn every_action_the_windows_name_exists() {
        let names = named_in_the_windows();
        assert!(names.len() > 15, "{names:?}");
        assert!(names.contains(&"apply-clip"), "{names:?}");
        for name in names {
            assert!(action_named(name).is_some(), "{name}");
        }
    }

    #[test]
    fn every_named_action_has_a_shortcut() {
        for name in named_in_the_windows() {
            let action = action_named(name).unwrap();
            assert!(
                SHORTCUTS.iter().any(|shortcut| shortcut.action == action),
                "{action:?} has no shortcut"
            );
        }
    }
}
