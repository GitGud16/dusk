//! The keys in use (docs/ARCHITECTURE.md, "Keyboard and settings"): every action's default
//! keys from the shortcut table, with the user's changes from `shortcuts.txt` over them; how
//! keys are written in that file and shown in the shortcut list; and which action a key press
//! stands for.

use std::fmt;

use slint::platform::Key;

use crate::shortcuts::{ACTIONS, Action};

/// The keys written by name, and their names in the shortcuts file and the shortcut list. A
/// key's first name is the one written; the others are read too.
const NAMED: &[(&str, KeyName)] = &[
    ("Space", KeyName::Char(' ')),
    ("Enter", KeyName::Named(Key::Return)),
    ("Return", KeyName::Named(Key::Return)),
    ("Esc", KeyName::Named(Key::Escape)),
    ("Escape", KeyName::Named(Key::Escape)),
    ("Tab", KeyName::Named(Key::Tab)),
    ("Backspace", KeyName::Named(Key::Backspace)),
    ("Delete", KeyName::Named(Key::Delete)),
    ("Insert", KeyName::Named(Key::Insert)),
    ("Home", KeyName::Named(Key::Home)),
    ("End", KeyName::Named(Key::End)),
    ("PageUp", KeyName::Named(Key::PageUp)),
    ("PageDown", KeyName::Named(Key::PageDown)),
    ("Left", KeyName::Named(Key::LeftArrow)),
    ("Right", KeyName::Named(Key::RightArrow)),
    ("Up", KeyName::Named(Key::UpArrow)),
    ("Down", KeyName::Named(Key::DownArrow)),
    ("F1", KeyName::Named(Key::F1)),
    ("F2", KeyName::Named(Key::F2)),
    ("F3", KeyName::Named(Key::F3)),
    ("F4", KeyName::Named(Key::F4)),
    ("F5", KeyName::Named(Key::F5)),
    ("F6", KeyName::Named(Key::F6)),
    ("F7", KeyName::Named(Key::F7)),
    ("F8", KeyName::Named(Key::F8)),
    ("F9", KeyName::Named(Key::F9)),
    ("F10", KeyName::Named(Key::F10)),
    ("F11", KeyName::Named(Key::F11)),
    ("F12", KeyName::Named(Key::F12)),
    // These two set keys and modifiers apart in the file.
    ("Comma", KeyName::Char(',')),
    ("Plus", KeyName::Char('+')),
];

/// The key `text` names, in any case: a name above, or a letter, a digit or a punctuation
/// character.
fn key_named(text: &str) -> Option<KeyName> {
    if let Some((_, key)) = NAMED
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(text))
    {
        return Some(*key);
    }
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() || c.is_ascii_punctuation() => {
            Some(KeyName::Char(c.to_ascii_lowercase()))
        }
        _ => None,
    }
}

/// What a punctuation key types with Shift on a US layout.
fn us_shifted(c: char) -> char {
    match c {
        '`' => '~',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        other => other,
    }
}

/// The header of the shortcuts file.
const HEADER: &str = "\
# Dusk's keyboard shortcuts. Each line gives an action its keys: one, several set apart
# by commas, or none after the \"=\". Keys are written like Ctrl+Shift+Z, Space, Left, F2
# or Comma. A line that starts with # is left alone: the actions you have not changed are
# listed that way, with their default keys, so taking the # away lets you change one. Dusk
# reads this file when it starts, and writes it when you change keys in the shortcut list.
";

/// A key without its modifiers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyName {
    /// A letter (lowercase), a digit, a punctuation character or the space bar.
    Char(char),
    /// A key without a character, such as an arrow.
    Named(Key),
}

/// A key with its modifiers, as pressed or as a shortcut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keys {
    pub key: KeyName,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Keys {
    /// The key of `c`, without modifiers.
    pub const fn char(c: char) -> Keys {
        Keys {
            key: KeyName::Char(c),
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    /// The named key `key`, without modifiers.
    pub const fn named(key: Key) -> Keys {
        Keys {
            key: KeyName::Named(key),
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    /// With Ctrl.
    pub const fn ctrl(self) -> Keys {
        Keys { ctrl: true, ..self }
    }

    /// With Shift, unless the key is punctuation, which says itself whether Shift was held:
    /// which punctuation needs Shift differs between keyboard layouts.
    pub const fn shift(self) -> Keys {
        let punctuation = matches!(self.key, KeyName::Char(c) if c.is_ascii_punctuation());
        Keys {
            shift: !punctuation,
            ..self
        }
    }

    /// With Alt.
    pub const fn alt(self) -> Keys {
        Keys { alt: true, ..self }
    }

    /// The keys `text` writes, such as `Ctrl+Shift+Z`, in any case and any order of
    /// modifiers; why not when it does not write any.
    pub fn parse(text: &str) -> Result<Keys, String> {
        let text = text.trim();
        let unknown = || format!("\"{text}\" is not a key Dusk knows");
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let Some((last, modifiers)) = parts.split_last() else {
            return Err(unknown());
        };
        if last.is_empty() {
            return Err(format!("\"{text}\" is missing its key"));
        }
        let mut keys = Keys {
            key: key_named(last).ok_or_else(unknown)?,
            ctrl: false,
            shift: false,
            alt: false,
        };
        for modifier in modifiers {
            match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => keys.ctrl = true,
                "alt" => keys.alt = true,
                "shift" => keys = keys.shift(),
                _ => return Err(unknown()),
            }
        }
        Ok(keys)
    }

    /// The keys of a key press: `text`, the text Slint reports for it, and `key`, the
    /// character of the key's place on a US layout (see `platform::pressed_key`). Letters
    /// and digits go by the key's place, so they work under any layout; punctuation goes by
    /// the character typed, or, when the layout types something else there (Arabic types ؟
    /// for ?), by what the key types on a US layout.
    pub fn pressed(
        text: &str,
        key: Option<char>,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> Option<Keys> {
        let mut chars = text.chars();
        let typed = match (chars.next(), chars.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        };
        let name = match (key, typed) {
            (Some(k), _) if k.is_ascii_alphanumeric() => KeyName::Char(k.to_ascii_lowercase()),
            (_, Some(c)) if c.is_ascii_punctuation() => KeyName::Char(c),
            (Some(k), _) if k.is_ascii_punctuation() => {
                KeyName::Char(if shift { us_shifted(k) } else { k })
            }
            (_, Some(c)) if c.is_ascii_alphanumeric() || c == ' ' => {
                KeyName::Char(c.to_ascii_lowercase())
            }
            (_, Some(c)) => NAMED.iter().find_map(|(_, name)| match name {
                KeyName::Named(named) if char::from(*named) == c => Some(*name),
                _ => None,
            })?,
            (_, None) => return None,
        };
        let keys = Keys {
            key: name,
            ctrl,
            shift: false,
            alt,
        };
        Some(if shift { keys.shift() } else { keys })
    }
}

impl fmt::Display for Keys {
    /// As the shortcut list and the shortcuts file write it, such as `Ctrl+Shift+Z`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        match NAMED.iter().find(|(_, key)| *key == self.key) {
            Some((name, _)) => f.write_str(name),
            None => match self.key {
                KeyName::Char(c) => write!(f, "{}", c.to_ascii_uppercase()),
                KeyName::Named(key) => write!(f, "{key:?}"),
            },
        }
    }
}

/// The keys of every action: the defaults, with the user's changes over them.
#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    keys: Vec<(Action, Vec<Keys>)>,
}

impl Default for Keymap {
    /// Every action with its default keys.
    fn default() -> Keymap {
        Keymap {
            keys: ACTIONS
                .iter()
                .map(|info| (info.action, info.keys.to_vec()))
                .collect(),
        }
    }
}

impl Keymap {
    /// The defaults with the keys `text`, a shortcuts file, gives actions over them, and
    /// what in it could not be read, a line each: a line Dusk cannot read is skipped, and its
    /// action keeps its default. A key a line gives one action is taken from the action
    /// that has it by default; a key an earlier line gave is not taken again.
    pub fn read(text: &str) -> (Keymap, Vec<String>) {
        let mut problems = Vec::new();
        // The lines that hold: the action, its keys and the line's number.
        let mut chosen: Vec<(Action, Vec<Keys>, usize)> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            let line = line.trim_start_matches('\u{feff}').trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, list)) = line.split_once('=') else {
                problems.push(format!(
                    "line {number}: an action's name, \"=\" and its keys were expected"
                ));
                continue;
            };
            let name = name.trim();
            let Some(action) = Action::named(name) else {
                problems.push(format!(
                    "line {number}: Dusk has no action called \"{name}\""
                ));
                continue;
            };
            if let Some((_, _, first)) = chosen.iter().find(|(earlier, _, _)| *earlier == action) {
                problems.push(format!(
                    "line {number}: {name} has its keys on line {first} already"
                ));
                continue;
            }
            let parsed: Result<Vec<Keys>, String> = list
                .split(',')
                .map(str::trim)
                .filter(|keys| !keys.is_empty())
                .map(Keys::parse)
                .collect();
            let keys = match parsed {
                Ok(parsed) => parsed.into_iter().fold(Vec::new(), |mut keys, key| {
                    if !keys.contains(&key) {
                        keys.push(key);
                    }
                    keys
                }),
                Err(why) => {
                    problems.push(format!("line {number}: {why}"));
                    continue;
                }
            };
            let taken = keys.iter().find_map(|key| {
                chosen
                    .iter()
                    .find(|(_, earlier, _)| earlier.contains(key))
                    .map(|(other, _, line)| (key, *other, *line))
            });
            if let Some((key, other, line)) = taken {
                let other = other.info().map_or("another action", |info| info.name);
                problems.push(format!(
                    "line {number}: {key} is {other}'s already, on line {line}"
                ));
                continue;
            }
            chosen.push((action, keys, number));
        }
        let taken: Vec<Keys> = chosen
            .iter()
            .flat_map(|(_, keys, _)| keys.iter().copied())
            .collect();
        let mut map = Keymap::default();
        for (action, keys) in &mut map.keys {
            match chosen.iter().find(|(chosen, _, _)| chosen == action) {
                Some((_, chosen, _)) => keys.clone_from(chosen),
                None => keys.retain(|key| !taken.contains(key)),
            }
        }
        (map, problems)
    }

    /// The shortcuts file that keeps this keymap: the actions whose keys differ from their
    /// defaults, and every other action as a comment with its default keys, by group.
    pub fn to_file(&self) -> String {
        let mut file = String::from(HEADER);
        let mut group = None;
        for info in ACTIONS {
            if group != Some(info.group) {
                file.push_str(&format!("\n# {}\n", info.group.title()));
                group = Some(info.group);
            }
            let keys = self.keys_of(info.action);
            let changed = keys != info.keys;
            let comment = if changed { "" } else { "# " };
            let list = keys
                .iter()
                .map(Keys::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            if list.is_empty() {
                file.push_str(&format!("{comment}{} =\n", info.name));
            } else {
                file.push_str(&format!("{comment}{} = {list}\n", info.name));
            }
        }
        file
    }

    /// The keys `action` has.
    pub fn keys_of(&self, action: Action) -> &[Keys] {
        self.keys
            .iter()
            .find(|(each, _)| *each == action)
            .map_or(&[], |(_, keys)| keys.as_slice())
    }

    /// The action `keys` stand for.
    pub fn action_for(&self, keys: &Keys) -> Option<Action> {
        self.keys
            .iter()
            .find(|(_, each)| each.contains(keys))
            .map(|(action, _)| *action)
    }

    /// The shortcut list as the windows show it: every action, with its keys, by group.
    pub fn shortcut_list(&self) -> slint::ModelRc<crate::ShortcutView> {
        let list: Vec<crate::ShortcutView> = ACTIONS
            .iter()
            .enumerate()
            .map(|(index, info)| crate::ShortcutView {
                heading: if index == 0 || ACTIONS[index - 1].group != info.group {
                    info.group.title().into()
                } else {
                    Default::default()
                },
                keys: self
                    .keys_of(info.action)
                    .iter()
                    .map(Keys::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into(),
                description: info.description.into(),
            })
            .collect();
        std::rc::Rc::new(slint::VecModel::from(list)).into()
    }

    /// The action a key press stands for (see [`Keys::pressed`]).
    pub fn action_for_press(
        &self,
        text: &str,
        key: Option<char>,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> Option<Action> {
        Keys::pressed(text, key, ctrl, shift, alt).and_then(|keys| self.action_for(&keys))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Keys {
        Keys::parse(text).unwrap_or_else(|why| panic!("{text}: {why}"))
    }

    fn named(key: Key) -> String {
        slint::SharedString::from(key).to_string()
    }

    #[test]
    fn keys_are_written_as_the_shortcut_list_shows_them() {
        for text in [
            "Space",
            "J",
            "Shift+J",
            "Left",
            "Up",
            "Home",
            "End",
            "PageDown",
            "Ctrl+Z",
            "Ctrl+Shift+Z",
            "Ctrl+Alt+1",
            "Alt+Delete",
            "Esc",
            "Enter",
            "Ctrl+Enter",
            "Tab",
            "Backspace",
            "Insert",
            "F2",
            "F12",
            "?",
            "=",
            "\\",
            "Ctrl+Comma",
            "Plus",
        ] {
            assert_eq!(keys(text).to_string(), text);
        }
        assert_eq!(Keys::char('z').ctrl().shift().to_string(), "Ctrl+Shift+Z");
        assert_eq!(Keys::named(Key::Return).ctrl(), keys("Ctrl+Enter"));
    }

    #[test]
    fn keys_are_read_in_any_case_and_any_order() {
        assert_eq!(keys("ctrl+shift+z"), keys("Ctrl+Shift+Z"));
        assert_eq!(keys("Shift+Ctrl+Z"), keys("Ctrl+Shift+Z"));
        assert_eq!(keys(" Ctrl + L "), keys("Ctrl+L"));
        assert_eq!(keys("Escape"), keys("Esc"));
        assert_eq!(keys("Return"), keys("Enter"));
        assert_eq!(keys("Control+S"), keys("Ctrl+S"));
        assert_eq!(keys(","), keys("Comma"));
    }

    #[test]
    fn punctuation_takes_no_shift() {
        // Which punctuation needs Shift differs between keyboard layouts.
        assert_eq!(keys("Shift+?"), keys("?"));
        assert_eq!(keys("Shift+?").to_string(), "?");
        assert_eq!(Keys::char('?').shift(), Keys::char('?'));
    }

    #[test]
    fn what_is_not_a_key_says_so() {
        for text in [
            "",
            "Ctrl+",
            "Ctrl",
            "Shift+Alt",
            "Hyper+X",
            "Ctrl+XY",
            "F13",
            "Ctrl++",
        ] {
            assert!(Keys::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn the_defaults_are_the_tables() {
        let map = Keymap::default();
        assert_eq!(map.action_for(&keys("Space")), Some(Action::PlayPause));
        assert_eq!(map.action_for(&keys("Ctrl+Y")), Some(Action::Redo));
        assert_eq!(map.action_for(&keys("Ctrl+Shift+Z")), Some(Action::Redo));
        assert_eq!(map.keys_of(Action::Redo).len(), 2);
        assert_eq!(
            map.action_for(&keys("Ctrl+Alt+3")),
            Some(Action::ToggleLock(2))
        );
        assert_eq!(map.action_for(&keys("X")), None);
    }

    fn press(
        map: &Keymap,
        text: &str,
        key: Option<char>,
        mods: (bool, bool, bool),
    ) -> Option<Action> {
        map.action_for_press(text, key, mods.0, mods.1, mods.2)
    }

    const PLAIN: (bool, bool, bool) = (false, false, false);
    const CTRL: (bool, bool, bool) = (true, false, false);
    const SHIFT: (bool, bool, bool) = (false, true, false);
    const ALT: (bool, bool, bool) = (false, false, true);
    const CTRL_SHIFT: (bool, bool, bool) = (true, true, false);
    const CTRL_ALT: (bool, bool, bool) = (true, false, true);

    #[test]
    fn plain_keys_find_their_action() {
        let map = Keymap::default();
        assert_eq!(press(&map, " ", None, PLAIN), Some(Action::PlayPause));
        assert_eq!(press(&map, "l", None, PLAIN), Some(Action::PlayForward));
        assert_eq!(
            press(&map, &named(Key::LeftArrow), None, PLAIN),
            Some(Action::StepBack)
        );
        assert_eq!(
            press(&map, &named(Key::End), None, PLAIN),
            Some(Action::GoToEnd)
        );
        assert_eq!(press(&map, "s", None, PLAIN), Some(Action::Split));
        assert_eq!(press(&map, "x", None, PLAIN), None);
    }

    #[test]
    fn letters_match_in_either_case_and_modifiers_must_match() {
        let map = Keymap::default();
        assert_eq!(press(&map, "L", None, PLAIN), Some(Action::PlayForward));
        assert_eq!(press(&map, "Z", None, CTRL_SHIFT), Some(Action::Redo));
        assert_eq!(press(&map, "z", None, CTRL), Some(Action::Undo));
        assert_eq!(press(&map, "z", None, PLAIN), None);
        assert_eq!(press(&map, "l", None, CTRL), Some(Action::Unlink));
        assert_eq!(press(&map, "l", None, ALT), None);
        assert_eq!(press(&map, "L", None, SHIFT), Some(Action::PlaySlowForward));
        assert_eq!(press(&map, "s", None, CTRL_SHIFT), Some(Action::SaveAs));
        assert_eq!(
            press(&map, "1", None, CTRL_ALT),
            Some(Action::ToggleLock(0))
        );
        assert_eq!(press(&map, "3", None, ALT), Some(Action::ToggleMute(2)));
        let delete = named(Key::Delete);
        assert_eq!(press(&map, &delete, None, PLAIN), Some(Action::Delete));
        assert_eq!(
            press(&map, &delete, None, SHIFT),
            Some(Action::RippleDelete)
        );
        assert_eq!(press(&map, &delete, None, ALT), Some(Action::DeleteOne));
        let enter = named(Key::Return);
        assert_eq!(
            press(&map, &enter, None, PLAIN),
            Some(Action::OpenClipEditor)
        );
        assert_eq!(press(&map, &enter, None, CTRL), Some(Action::ApplyClip));
    }

    #[test]
    fn letter_keys_work_under_any_keyboard_layout() {
        // Arabic (101): L types م, and Shift+L a slash; the key is L either way.
        let map = Keymap::default();
        assert_eq!(
            press(&map, "م", Some('l'), PLAIN),
            Some(Action::PlayForward)
        );
        assert_eq!(
            press(&map, "/", Some('l'), SHIFT),
            Some(Action::PlaySlowForward)
        );
        assert_eq!(press(&map, "س", Some('s'), CTRL), Some(Action::Save));
        assert_eq!(press(&map, " ", None, PLAIN), Some(Action::PlayPause));
    }

    #[test]
    fn punctuation_keys_work_with_or_without_shift_under_any_layout() {
        let map = Keymap::default();
        // "?" needs Shift on most layouts; which key it is varies.
        assert_eq!(
            press(&map, "?", Some('/'), SHIFT),
            Some(Action::ShortcutList)
        );
        assert_eq!(press(&map, "?", None, PLAIN), Some(Action::ShortcutList));
        assert_eq!(press(&map, "=", Some('='), PLAIN), Some(Action::ZoomIn));
        // Arabic (101) types ؟ there: the key's character on a US layout stands in.
        assert_eq!(
            press(&map, "؟", Some('/'), SHIFT),
            Some(Action::ShortcutList)
        );
        // A layout that types the character elsewhere is taken at its word.
        assert_eq!(
            press(&map, "?", Some('-'), SHIFT),
            Some(Action::ShortcutList)
        );
    }

    #[test]
    fn keys_edit_without_the_mouse() {
        let map = Keymap::default();
        let up = named(Key::UpArrow);
        let down = named(Key::DownArrow);
        assert_eq!(press(&map, &up, None, PLAIN), Some(Action::PreviousCut));
        assert_eq!(press(&map, &down, None, PLAIN), Some(Action::NextCut));
        assert_eq!(
            press(&map, &named(Key::LeftArrow), None, SHIFT),
            Some(Action::BackSecond)
        );
        assert_eq!(
            press(&map, &named(Key::RightArrow), None, SHIFT),
            Some(Action::AheadSecond)
        );
        assert_eq!(
            press(&map, "d", Some('d'), PLAIN),
            Some(Action::SelectAtPlayhead)
        );
        assert_eq!(
            press(&map, "A", Some('a'), CTRL_SHIFT),
            Some(Action::SelectNone)
        );
        assert_eq!(press(&map, &up, None, ALT), Some(Action::PreviousMedia));
        assert_eq!(press(&map, &down, None, ALT), Some(Action::NextMedia));
        // I and O start and end a clip in either window.
        assert_eq!(press(&map, "i", Some('i'), PLAIN), Some(Action::MarkIn));
        assert_eq!(press(&map, "o", Some('o'), PLAIN), Some(Action::MarkOut));
    }

    #[test]
    fn a_file_changes_only_the_actions_it_names() {
        let (map, problems) = Keymap::read("split = X\nredo = Ctrl+Y\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(map.action_for(&keys("X")), Some(Action::Split));
        assert_eq!(map.action_for(&keys("S")), None);
        assert_eq!(map.action_for(&keys("Ctrl+Shift+Z")), None);
        assert_eq!(map.action_for(&keys("Ctrl+Y")), Some(Action::Redo));
        assert_eq!(map.action_for(&keys("Space")), Some(Action::PlayPause));
    }

    #[test]
    fn an_action_takes_several_keys_or_none() {
        let (map, problems) = Keymap::read("undo = Ctrl+Z, Alt+Backspace\nunlink =\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            map.keys_of(Action::Undo),
            [keys("Ctrl+Z"), keys("Alt+Backspace")]
        );
        assert!(map.keys_of(Action::Unlink).is_empty());
        assert_eq!(map.action_for(&keys("Ctrl+L")), None);
    }

    #[test]
    fn a_key_given_to_another_action_leaves_its_default() {
        let (map, problems) = Keymap::read("split = Delete\n");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(map.action_for(&keys("Delete")), Some(Action::Split));
        assert!(map.keys_of(Action::Delete).is_empty());
    }

    #[test]
    fn comments_blank_lines_and_spaces_are_left_alone() {
        let text = "\u{feff}# my keys\r\n\r\n   split   =   X   \r\n# undo = Ctrl+U\r\n";
        let (map, problems) = Keymap::read(text);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(map.action_for(&keys("X")), Some(Action::Split));
        assert_eq!(map.action_for(&keys("Ctrl+Z")), Some(Action::Undo));
    }

    #[test]
    fn lines_that_cannot_be_read_are_skipped_and_said() {
        let text = "splitt = X\n\
                    split = Hyper+X\n\
                    undo\n\
                    save = Ctrl+L\n\
                    unlink = Ctrl+L\n\
                    split = Q\n\
                    split = W\n";
        let (map, problems) = Keymap::read(text);
        assert_eq!(problems.len(), 5, "{problems:?}");
        for (problem, line) in problems.iter().zip([1, 2, 3, 5, 7]) {
            assert!(problem.starts_with(&format!("line {line}: ")), "{problem}");
        }
        assert!(problems[0].contains("splitt"), "{}", problems[0]);
        assert!(
            problems[3].contains("save") && problems[3].contains("line 4"),
            "{}",
            problems[3]
        );
        assert!(problems[4].contains("line 6"), "{}", problems[4]);
        // Line 4 holds; line 5 is skipped, so unlink is left without its default.
        assert_eq!(map.action_for(&keys("Ctrl+L")), Some(Action::Save));
        assert!(map.keys_of(Action::Unlink).is_empty());
        // The first line split could use holds; the line after it is skipped.
        assert_eq!(map.keys_of(Action::Split), [keys("Q")]);
        assert_eq!(map.action_for(&keys("W")), None);
    }

    #[test]
    fn the_file_keeps_the_changes_and_lists_every_other_action() {
        let defaults = Keymap::default().to_file();
        assert!(
            defaults
                .lines()
                .all(|line| line.is_empty() || line.starts_with('#')),
            "{defaults}"
        );
        assert!(
            defaults.contains("# redo = Ctrl+Shift+Z, Ctrl+Y\n"),
            "{defaults}"
        );
        assert!(defaults.contains("# mute-a1 = Alt+3\n"), "{defaults}");
        let (changed, problems) =
            Keymap::read("split = X\nunlink =\nundo = Ctrl+Z, Alt+Backspace\n");
        assert!(problems.is_empty(), "{problems:?}");
        let file = changed.to_file();
        assert!(file.contains("\nsplit = X\n"), "{file}");
        assert!(file.contains("\nunlink =\n"), "{file}");
        assert!(file.contains("\nundo = Ctrl+Z, Alt+Backspace\n"), "{file}");
        assert!(file.contains("\n# delete = Delete\n"), "{file}");
        // What Dusk writes, it reads back the same.
        assert_eq!(Keymap::read(&file), (changed, Vec::new()));
        assert_eq!(Keymap::read(&defaults), (Keymap::default(), Vec::new()));
    }
}
