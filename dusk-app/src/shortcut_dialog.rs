//! The shortcut list's dialog (docs/ARCHITECTURE.md, "Keyboard and settings"): narrowed to
//! what is typed, an action picked with Up and Down or a click, and its keys changed by
//! pressing new ones: keys another action has are taken from it only when pressed a second
//! time. What it says under the list is worked out here; the window shows it.

use slint::{SharedString, VecModel};

use crate::ShortcutView;
use crate::app::{App, with_app};
use crate::keymap::{Keymap, Keys};
use crate::settings;
use crate::shortcuts::Action;

/// The shortcut list while it is open.
#[derive(Debug, Default)]
pub struct ShortcutDialog {
    /// What is typed in the search field.
    pub query: String,
    /// The action picked in the list.
    pub selected: Option<Action>,
    /// Waiting for the picked action's new keys.
    pub capturing: bool,
    /// Keys another action has, pressed once while capturing.
    pending: Option<Keys>,
    /// What the dialog says under the list.
    pub note: String,
}

impl ShortcutDialog {
    /// The dialog as it opens: the whole list, its first action picked.
    pub fn open(map: &Keymap) -> ShortcutDialog {
        let mut dialog = ShortcutDialog::default();
        dialog.search(map, "");
        dialog
    }

    /// What is typed changed: the pick stays when it is still listed, otherwise the first
    /// listed action is picked.
    pub fn search(&mut self, map: &Keymap, query: &str) {
        query.clone_into(&mut self.query);
        let listed = map.matching(query);
        if !self.selected.is_some_and(|picked| listed.contains(&picked)) {
            self.selected = listed.first().copied();
        }
    }

    /// Up or Down: the pick moves `by` rows, staying at either end.
    pub fn move_pick(&mut self, map: &Keymap, by: isize) {
        let listed = map.matching(&self.query);
        let Some(last) = listed.len().checked_sub(1) else {
            self.selected = None;
            return;
        };
        let at = self
            .selected
            .and_then(|picked| listed.iter().position(|action| *action == picked))
            .unwrap_or(0);
        self.selected = Some(listed[at.saturating_add_signed(by).min(last)]);
    }

    /// The row at `index` was clicked.
    pub fn pick(&mut self, map: &Keymap, index: usize) {
        if let Some(action) = map.matching(&self.query).get(index) {
            self.selected = Some(*action);
        }
    }

    /// The picked action's row in the list as narrowed.
    pub fn picked_row(&self, map: &Keymap) -> Option<usize> {
        let picked = self.selected?;
        map.matching(&self.query)
            .iter()
            .position(|action| *action == picked)
    }

    /// Enter or Change keys: waits for the picked action's new keys.
    pub fn start_capture(&mut self) {
        let Some(action) = self.selected else {
            return;
        };
        self.capturing = true;
        self.pending = None;
        self.note = format!(
            "Press the new keys for “{}”, or Esc to keep its keys.",
            described(action)
        );
    }

    /// Escape while waiting: stops waiting.
    pub fn stop_capture(&mut self) {
        self.capturing = false;
        self.pending = None;
        self.note.clear();
    }

    /// Keys pressed while waiting: given to the picked action, unless another action has
    /// them and they were not pressed just before. True when the keymap changed.
    pub fn capture(&mut self, map: &mut Keymap, keys: Keys) -> bool {
        let Some(action) = self.selected.filter(|_| self.capturing) else {
            return false;
        };
        match map.action_for(&keys) {
            Some(own) if own == action => {
                self.stop_capture();
                self.note = format!("{keys} already does “{}”.", described(action));
                false
            }
            Some(other) if self.pending != Some(keys) => {
                self.pending = Some(keys);
                self.note = format!(
                    "{keys} does “{}”. Press it again to give it to this action, or press \
                     other keys.",
                    described(other)
                );
                false
            }
            _ => {
                let losers = map.set_keys(action, vec![keys]);
                self.capturing = false;
                self.pending = None;
                self.note = format!("{keys} now does “{}”.", described(action));
                self.note.push_str(&left_without_keys(map, &losers));
                true
            }
        }
    }

    /// Remove keys: the picked action has none. True when the keymap changed.
    pub fn remove(&mut self, map: &mut Keymap) -> bool {
        let Some(action) = self
            .selected
            .filter(|action| !map.keys_of(*action).is_empty())
        else {
            return false;
        };
        map.set_keys(action, Vec::new());
        self.stop_capture();
        self.note = format!("“{}” has no keys now.", described(action));
        true
    }

    /// Default keys: the picked action has its own again. True when the keymap changed.
    pub fn restore(&mut self, map: &mut Keymap) -> bool {
        let Some(info) = self.selected.and_then(Action::info) else {
            return false;
        };
        if map.keys_of(info.action) == info.keys {
            return false;
        }
        let losers = map.restore(info.action);
        self.stop_capture();
        self.note = format!("“{}” has its default keys again.", info.description);
        self.note.push_str(&left_without_keys(map, &losers));
        true
    }

    /// All defaults: every action has its own keys again. True when the keymap changed.
    pub fn restore_all(&mut self, map: &mut Keymap) -> bool {
        if *map == Keymap::default() {
            return false;
        }
        *map = Keymap::default();
        self.stop_capture();
        self.note = "Every action has its default keys again.".to_owned();
        true
    }
}

/// What `action` does, as the list says it.
fn described(action: Action) -> &'static str {
    action.info().map_or("", |info| info.description)
}

/// What the note adds for the actions among `losers` that have no keys left.
fn left_without_keys(map: &Keymap, losers: &[Action]) -> String {
    losers
        .iter()
        .filter(|loser| map.keys_of(**loser).is_empty())
        .map(|loser| format!(" “{}” has no keys now.", described(*loser)))
        .collect()
}

impl App {
    /// `?`, the toolbar's Shortcuts and Help → Keyboard shortcuts: the shortcut list opens,
    /// its search field taking the keys.
    pub(crate) fn open_shortcut_list(&mut self) {
        if self.shortcut_dialog.is_none() {
            self.shortcut_dialog = Some(ShortcutDialog::open(&self.keymap));
        }
        self.refresh_shortcut_dialog();
    }

    pub fn shortcut_search(&mut self, text: &str) {
        if let Some(dialog) = &mut self.shortcut_dialog {
            dialog.search(&self.keymap, text);
        }
        self.refresh_shortcut_dialog();
    }

    pub fn shortcut_pick(&mut self, row: i32) {
        if let (Some(dialog), Ok(row)) = (&mut self.shortcut_dialog, usize::try_from(row)) {
            dialog.pick(&self.keymap, row);
        }
        self.refresh_shortcut_dialog();
    }

    /// Change keys: the next keys pressed are the picked action's, so the keys leave the
    /// search field meanwhile.
    pub fn shortcut_change(&mut self) {
        if let Some(dialog) = &mut self.shortcut_dialog {
            dialog.start_capture();
            if dialog.capturing
                && let Some(window) = self.window()
            {
                window.invoke_take_keys();
            }
        }
        self.refresh_shortcut_dialog();
    }

    pub fn shortcut_remove(&mut self) {
        let changed = self
            .shortcut_dialog
            .as_mut()
            .is_some_and(|dialog| dialog.remove(&mut self.keymap));
        self.after_shortcut_change(changed);
    }

    pub fn shortcut_restore(&mut self) {
        let changed = self
            .shortcut_dialog
            .as_mut()
            .is_some_and(|dialog| dialog.restore(&mut self.keymap));
        self.after_shortcut_change(changed);
    }

    pub fn shortcut_restore_all(&mut self) {
        let changed = self
            .shortcut_dialog
            .as_mut()
            .is_some_and(|dialog| dialog.restore_all(&mut self.keymap));
        self.after_shortcut_change(changed);
    }

    pub fn shortcut_close(&mut self) {
        self.shortcut_dialog = None;
        if let Some(window) = self.window() {
            window.set_shortcuts_open(false);
        }
    }

    /// A key pressed while the shortcut list is open: the picked action's new keys while it
    /// waits for them, otherwise Up and Down pick, Enter waits for keys and Escape closes.
    pub(crate) fn shortcut_dialog_key(
        &mut self,
        text: &str,
        key: Option<char>,
        ctrl: bool,
        shift: bool,
        alt: bool,
    ) -> bool {
        use slint::platform::Key;
        let named = |key: Key| text == SharedString::from(key).as_str();
        let Some(dialog) = &mut self.shortcut_dialog else {
            return false;
        };
        if dialog.capturing {
            if named(Key::Escape) {
                dialog.stop_capture();
            } else if let Some(keys) = Keys::pressed(text, key, ctrl, shift, alt) {
                let changed = dialog.capture(&mut self.keymap, keys);
                return self.after_shortcut_change(changed);
            }
            self.refresh_shortcut_dialog();
            return true;
        }
        if named(Key::UpArrow) || named(Key::DownArrow) {
            dialog.move_pick(&self.keymap, if named(Key::UpArrow) { -1 } else { 1 });
            self.refresh_shortcut_dialog();
        } else if named(Key::Return) {
            self.shortcut_change();
        } else if named(Key::Escape) {
            self.shortcut_close();
        } else {
            return false;
        }
        true
    }

    /// After a change in the dialog: when the keys changed, the shortcuts file is written and
    /// what Dusk could not read in it is gone; the list shows the keys as they are now.
    fn after_shortcut_change(&mut self, changed: bool) -> bool {
        if changed {
            self.keymap_problem.clear();
            self.save_keymap();
        }
        self.refresh_shortcut_dialog();
        true
    }

    /// Writes the shortcuts file on the file worker, saying so if it cannot.
    fn save_keymap(&self) {
        let Some(dir) = self.settings_dir.clone() else {
            return;
        };
        let keymap = self.keymap.clone();
        self.files.run(move || {
            if let Err(error) = settings::write_keymap(&dir, &keymap) {
                let message = format!(
                    "Dusk could not save the shortcuts in {}: {error}. They last until Dusk \
                     closes.",
                    dir.display()
                );
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| app.fail(&message));
                });
            }
        });
    }

    /// Shows the shortcut list as it stands, in the dialog and in the clip editor's list.
    pub(crate) fn refresh_shortcut_dialog(&self) {
        if let Some(window) = &self.editor_window {
            window.set_shortcuts(self.keymap.shortcut_list());
            window.set_shortcut_problem(self.keymap_problem.clone().into());
        }
        let Some(window) = self.window() else {
            return;
        };
        window.set_shortcut_problem(self.keymap_problem.clone().into());
        let Some(dialog) = &self.shortcut_dialog else {
            return;
        };
        let rows = self.keymap.rows(&dialog.query);
        let picked = dialog.picked_row(&self.keymap);
        let headings_above = picked.map_or(0, |picked| {
            rows[..=picked]
                .iter()
                .filter(|row| row.heading.is_some())
                .count()
        });
        let views = crate::keymap::views(rows);
        window.set_shortcut_rows(std::rc::Rc::new(VecModel::from(views)).into());
        window.set_shortcut_picked(picked.and_then(|row| i32::try_from(row).ok()).unwrap_or(-1));
        window.set_shortcut_headings_above(i32::try_from(headings_above).unwrap_or(0));
        window.set_shortcut_note(dialog.note.clone().into());
        window.set_shortcut_capturing(dialog.capturing);
        window.set_shortcuts_open(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Keys {
        Keys::parse(text).unwrap()
    }

    #[test]
    fn it_opens_on_the_first_action_and_follows_the_search() {
        let map = Keymap::default();
        let mut dialog = ShortcutDialog::open(&map);
        assert_eq!(dialog.selected, Some(Action::PlayPause));
        assert_eq!(dialog.picked_row(&map), Some(0));
        dialog.search(&map, "delete");
        assert_eq!(dialog.selected, Some(Action::Delete));
        dialog.move_pick(&map, 1);
        assert_eq!(dialog.selected, Some(Action::RippleDelete));
        // The pick stays while it is still listed.
        dialog.search(&map, "ripple");
        assert_eq!(dialog.selected, Some(Action::RippleDelete));
        assert_eq!(dialog.picked_row(&map), Some(0));
        // Nothing listed, nothing picked.
        dialog.search(&map, "no such thing");
        assert_eq!(dialog.selected, None);
        dialog.move_pick(&map, 1);
        assert_eq!(dialog.selected, None);
    }

    #[test]
    fn up_and_down_stay_at_the_ends_and_a_click_picks() {
        let map = Keymap::default();
        let mut dialog = ShortcutDialog::open(&map);
        dialog.move_pick(&map, -1);
        assert_eq!(dialog.selected, Some(Action::PlayPause));
        dialog.search(&map, "delete");
        dialog.move_pick(&map, 5);
        assert_eq!(dialog.selected, Some(Action::DeleteOne));
        dialog.pick(&map, 1);
        assert_eq!(dialog.selected, Some(Action::RippleDelete));
        dialog.pick(&map, 7);
        assert_eq!(dialog.selected, Some(Action::RippleDelete));
    }

    #[test]
    fn new_keys_no_one_has_are_taken_at_once() {
        let mut map = Keymap::default();
        let mut dialog = ShortcutDialog::open(&map);
        dialog.search(&map, "split");
        dialog.start_capture();
        assert!(dialog.capturing);
        assert!(
            dialog.note.starts_with("Press the new keys for"),
            "{}",
            dialog.note
        );
        assert!(dialog.capture(&mut map, keys("X")));
        assert!(!dialog.capturing);
        assert_eq!(map.keys_of(Action::Split), [keys("X")]);
        assert!(dialog.note.starts_with("X now does"), "{}", dialog.note);
    }

    #[test]
    fn keys_another_action_has_are_taken_when_pressed_again() {
        let mut map = Keymap::default();
        let mut dialog = ShortcutDialog::open(&map);
        dialog.search(&map, "split");
        dialog.start_capture();
        assert!(!dialog.capture(&mut map, keys("Ctrl+L")));
        assert!(dialog.capturing);
        assert!(
            dialog.note.contains("Ctrl+L") && dialog.note.contains("again"),
            "{}",
            dialog.note
        );
        assert_eq!(map.keys_of(Action::Split), [keys("S")]);
        // Other keys in between start over.
        assert!(!dialog.capture(&mut map, keys("Ctrl+O")));
        assert!(!dialog.capture(&mut map, keys("Ctrl+L")));
        assert!(dialog.capture(&mut map, keys("Ctrl+L")));
        assert_eq!(map.keys_of(Action::Split), [keys("Ctrl+L")]);
        assert!(map.keys_of(Action::Unlink).is_empty());
        assert!(dialog.note.contains("no keys now"), "{}", dialog.note);
    }

    #[test]
    fn its_own_keys_pressed_again_change_nothing() {
        let mut map = Keymap::default();
        let mut dialog = ShortcutDialog::open(&map);
        dialog.search(&map, "split");
        dialog.start_capture();
        assert!(!dialog.capture(&mut map, keys("S")));
        assert!(!dialog.capturing);
        assert_eq!(map.keys_of(Action::Split), [keys("S")]);
    }

    #[test]
    fn escape_stops_waiting() {
        let mut map = Keymap::default();
        let mut dialog = ShortcutDialog::open(&map);
        dialog.start_capture();
        dialog.stop_capture();
        assert!(!dialog.capturing && dialog.note.is_empty());
        assert_eq!(map, Keymap::default());
        // Without a pick there is nothing to wait for.
        dialog.search(&map, "no such thing");
        dialog.start_capture();
        assert!(!dialog.capturing);
        assert!(!dialog.capture(&mut map, keys("X")));
    }

    #[test]
    fn keys_are_removed_and_given_back() {
        let mut map = Keymap::default();
        let mut dialog = ShortcutDialog::open(&map);
        dialog.search(&map, "split");
        assert!(dialog.remove(&mut map));
        assert!(map.keys_of(Action::Split).is_empty());
        assert!(!dialog.remove(&mut map));
        assert!(dialog.restore(&mut map));
        assert_eq!(map.keys_of(Action::Split), [keys("S")]);
        assert!(!dialog.restore(&mut map));
        dialog.capture(&mut map, keys("X"));
        map.set_keys(Action::Undo, vec![keys("Q")]);
        assert!(dialog.restore_all(&mut map));
        assert_eq!(map, Keymap::default());
        assert!(!dialog.restore_all(&mut map));
    }
}
