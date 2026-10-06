//! The central shortcut table (CLAUDE.md, "UI conventions"): every action, its name, what it
//! does and its default keys, so the shortcut list stays complete. Menu items and buttons
//! name the same actions. The keys in use, the defaults with the user's changes over them,
//! are the keymap's (`keymap`; docs/ARCHITECTURE.md, "Keyboard and settings").

use slint::platform::Key;

use crate::keymap::Keys;

/// Something the user can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// Compresses a video file into a smaller one, outside the project.
    CompressVideo,
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
    /// The clip editor's Export as file.
    ExportClip,
}

/// What the shortcut list groups actions under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Playback,
    Editing,
    ClipEditor,
    Tracks,
    View,
    Project,
}

impl Group {
    /// Its heading in the shortcut list and in the shortcuts file.
    pub fn title(self) -> &'static str {
        match self {
            Group::Playback => "Playback",
            Group::Editing => "Editing",
            Group::ClipEditor => "Clip editor",
            Group::Tracks => "Tracks",
            Group::View => "View",
            Group::Project => "Project",
        }
    }
}

/// An action's entry in the table.
#[derive(Debug)]
pub struct ActionInfo {
    pub action: Action,
    /// Its name in the shortcuts file, and the name menu items and buttons give it.
    pub name: &'static str,
    pub group: Group,
    /// What it does, for the shortcut list.
    pub description: &'static str,
    /// The keys it has unless the user changed them.
    pub keys: &'static [Keys],
}

const fn entry(
    action: Action,
    name: &'static str,
    group: Group,
    description: &'static str,
    keys: &'static [Keys],
) -> ActionInfo {
    ActionInfo {
        action,
        name,
        group,
        description,
        keys,
    }
}

const fn letter(c: char) -> Keys {
    Keys::char(c)
}

const fn named(key: Key) -> Keys {
    Keys::named(key)
}

/// Every action, in the order the shortcut list shows them.
pub const ACTIONS: &[ActionInfo] = &[
    entry(
        Action::PlayPause,
        "play-pause",
        Group::Playback,
        "Play or pause",
        &[letter(' ')],
    ),
    entry(
        Action::PlayBackward,
        "play-backward",
        Group::Playback,
        "Play backwards; press again to play faster, up to 32x",
        &[letter('j')],
    ),
    entry(
        Action::Pause,
        "pause",
        Group::Playback,
        "Pause",
        &[letter('k')],
    ),
    entry(
        Action::PlayForward,
        "play-forward",
        Group::Playback,
        "Play; press again to play faster, up to 32x",
        &[letter('l')],
    ),
    entry(
        Action::PlaySlowBackward,
        "play-slow-backward",
        Group::Playback,
        "Play backwards slowly; press again to play slower, down to 0.1x",
        &[letter('j').shift()],
    ),
    entry(
        Action::PlaySlowForward,
        "play-slow-forward",
        Group::Playback,
        "Play slowly; press again to play slower, down to 0.1x",
        &[letter('l').shift()],
    ),
    entry(
        Action::StepBack,
        "previous-frame",
        Group::Playback,
        "Previous frame",
        &[named(Key::LeftArrow)],
    ),
    entry(
        Action::StepForward,
        "next-frame",
        Group::Playback,
        "Next frame",
        &[named(Key::RightArrow)],
    ),
    entry(
        Action::GoToStart,
        "go-to-start",
        Group::Playback,
        "Go to the start",
        &[named(Key::Home)],
    ),
    entry(
        Action::GoToEnd,
        "go-to-end",
        Group::Playback,
        "Go to the last frame",
        &[named(Key::End)],
    ),
    entry(
        Action::Undo,
        "undo",
        Group::Editing,
        "Undo",
        &[letter('z').ctrl()],
    ),
    entry(
        Action::Redo,
        "redo",
        Group::Editing,
        "Redo",
        &[letter('z').ctrl().shift(), letter('y').ctrl()],
    ),
    entry(
        Action::Split,
        "split",
        Group::Editing,
        "Split at the playhead: the selected clip, or every clip there",
        &[letter('s')],
    ),
    entry(
        Action::Delete,
        "delete",
        Group::Editing,
        "Delete the selected clip and its linked clips, leaving a gap",
        &[named(Key::Delete)],
    ),
    entry(
        Action::RippleDelete,
        "ripple-delete",
        Group::Editing,
        "Delete and close the gap on every unlocked track",
        &[named(Key::Delete).shift()],
    ),
    entry(
        Action::DeleteOne,
        "delete-one",
        Group::Editing,
        "Delete the selected clip only, not its linked clips",
        &[named(Key::Delete).alt()],
    ),
    entry(
        Action::Unlink,
        "unlink",
        Group::Editing,
        "Detach audio: unlink the selected clip",
        &[letter('l').ctrl()],
    ),
    entry(
        Action::ToggleEnabled,
        "toggle-enabled",
        Group::Editing,
        "Enable or disable the selected clip",
        &[letter('e')],
    ),
    entry(
        Action::ToggleFill,
        "toggle-fill",
        Group::Editing,
        "Fit the picture with bars, or fill the frame and crop",
        &[letter('f')],
    ),
    entry(
        Action::Place,
        "place",
        Group::Editing,
        "Place the selected media at the playhead",
        &[letter('p')],
    ),
    entry(
        Action::OpenClipEditor,
        "open-clip-editor",
        Group::Editing,
        "Open the selected clip in the clip editor",
        &[named(Key::Return)],
    ),
    entry(
        Action::MarkIn,
        "mark-in",
        Group::ClipEditor,
        "Clip editor: start the clip at the playhead",
        &[letter('i')],
    ),
    entry(
        Action::MarkOut,
        "mark-out",
        Group::ClipEditor,
        "Clip editor: end the clip at the playhead",
        &[letter('o')],
    ),
    entry(
        Action::TurnRight,
        "turn-right",
        Group::ClipEditor,
        "Clip editor: turn the picture right",
        &[letter('r')],
    ),
    entry(
        Action::TurnLeft,
        "turn-left",
        Group::ClipEditor,
        "Clip editor: turn the picture left",
        &[letter('r').shift()],
    ),
    entry(
        Action::MirrorLeftRight,
        "mirror-left-right",
        Group::ClipEditor,
        "Clip editor: mirror the picture left to right",
        &[letter('h')],
    ),
    entry(
        Action::MirrorTopBottom,
        "mirror-top-bottom",
        Group::ClipEditor,
        "Clip editor: mirror the picture top to bottom",
        &[letter('v')],
    ),
    entry(
        Action::ApplyClip,
        "apply-clip",
        Group::ClipEditor,
        "Clip editor: apply the changes to the project",
        &[named(Key::Return).ctrl()],
    ),
    entry(
        Action::CloseClipEditor,
        "close-clip-editor",
        Group::ClipEditor,
        "Close the clip editor",
        &[letter('w').ctrl()],
    ),
    entry(
        Action::ReloadClip,
        "reload-clip",
        Group::ClipEditor,
        "Clip editor: the clip changed in the main window; take it as it is now",
        &[letter('r').ctrl()],
    ),
    entry(
        Action::KeepDraft,
        "keep-draft",
        Group::ClipEditor,
        "Clip editor: the clip changed in the main window; keep the draft",
        &[letter('k').ctrl()],
    ),
    entry(
        Action::ExportClip,
        "export-clip",
        Group::ClipEditor,
        "Clip editor: export the clip as a file of its own",
        &[letter('e').ctrl().shift()],
    ),
    entry(
        Action::ToggleMute(0),
        "mute-v1",
        Group::Tracks,
        "Hide or show V1",
        &[letter('1').alt()],
    ),
    entry(
        Action::ToggleMute(1),
        "mute-v2",
        Group::Tracks,
        "Hide or show V2",
        &[letter('2').alt()],
    ),
    entry(
        Action::ToggleMute(2),
        "mute-a1",
        Group::Tracks,
        "Mute or unmute A1",
        &[letter('3').alt()],
    ),
    entry(
        Action::ToggleMute(3),
        "mute-a2",
        Group::Tracks,
        "Mute or unmute A2",
        &[letter('4').alt()],
    ),
    entry(
        Action::ToggleLock(0),
        "lock-v1",
        Group::Tracks,
        "Lock or unlock V1",
        &[letter('1').ctrl().alt()],
    ),
    entry(
        Action::ToggleLock(1),
        "lock-v2",
        Group::Tracks,
        "Lock or unlock V2",
        &[letter('2').ctrl().alt()],
    ),
    entry(
        Action::ToggleLock(2),
        "lock-a1",
        Group::Tracks,
        "Lock or unlock A1",
        &[letter('3').ctrl().alt()],
    ),
    entry(
        Action::ToggleLock(3),
        "lock-a2",
        Group::Tracks,
        "Lock or unlock A2",
        &[letter('4').ctrl().alt()],
    ),
    entry(
        Action::ZoomIn,
        "zoom-in",
        Group::View,
        "Zoom in",
        &[letter('=')],
    ),
    entry(
        Action::ZoomOut,
        "zoom-out",
        Group::View,
        "Zoom out",
        &[letter('-')],
    ),
    entry(
        Action::ZoomFit,
        "zoom-fit",
        Group::View,
        "Fit the sequence in view",
        &[letter('\\')],
    ),
    entry(
        Action::ShortcutList,
        "shortcut-list",
        Group::View,
        "Show the keyboard shortcuts",
        &[letter('?')],
    ),
    entry(
        Action::NewProject,
        "new-project",
        Group::Project,
        "New project",
        &[letter('n').ctrl()],
    ),
    entry(
        Action::OpenProject,
        "open-project",
        Group::Project,
        "Open a project",
        &[letter('o').ctrl()],
    ),
    entry(
        Action::Save,
        "save",
        Group::Project,
        "Save the project",
        &[letter('s').ctrl()],
    ),
    entry(
        Action::SaveAs,
        "save-as",
        Group::Project,
        "Save the project under a new name",
        &[letter('s').ctrl().shift()],
    ),
    entry(
        Action::Import,
        "import",
        Group::Project,
        "Import media",
        &[letter('i').ctrl()],
    ),
    entry(
        Action::SequenceSettings,
        "sequence-settings",
        Group::Project,
        "Sequence settings: frame rate and size",
        &[letter('r').ctrl().shift()],
    ),
    entry(
        Action::Export,
        "export",
        Group::Project,
        "Export the timeline",
        &[letter('e').ctrl()],
    ),
    entry(
        Action::CancelExport,
        "cancel-export",
        Group::Project,
        "Cancel the export",
        &[named(Key::Escape)],
    ),
    entry(
        Action::CompressVideo,
        "compress-video",
        Group::Project,
        "Compress a video into a smaller file",
        &[letter('m').ctrl()],
    ),
    entry(
        Action::Quit,
        "quit",
        Group::Project,
        "Quit",
        &[letter('q').ctrl()],
    ),
];

impl Action {
    /// Its entry in the table; `None` only for a track that is not there, such as the
    /// fifth.
    pub fn info(self) -> Option<&'static ActionInfo> {
        ACTIONS.iter().find(|info| info.action == self)
    }

    /// The action called `name` in the shortcuts file, a menu item or a button.
    pub fn named(name: &str) -> Option<Action> {
        ACTIONS
            .iter()
            .find(|info| info.name == name)
            .map(|info| info.action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_name_of_its_own() {
        for (i, info) in ACTIONS.iter().enumerate() {
            assert!(
                info.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{}",
                info.name
            );
            assert_eq!(Action::named(info.name), Some(info.action));
            assert_eq!(info.action.info().map(|info| info.name), Some(info.name));
            for other in &ACTIONS[i + 1..] {
                assert_ne!(info.name, other.name);
                assert_ne!(info.action, other.action);
            }
        }
        assert_eq!(Action::named("mute-a1"), Some(Action::ToggleMute(2)));
        assert_eq!(Action::named("splitt"), None);
        assert!(Action::ToggleMute(4).info().is_none());
    }

    #[test]
    fn no_two_actions_share_a_default_key() {
        let keys: Vec<(Action, Keys)> = ACTIONS
            .iter()
            .flat_map(|info| info.keys.iter().map(move |keys| (info.action, *keys)))
            .collect();
        for (i, (action, key)) in keys.iter().enumerate() {
            for (other, other_key) in &keys[i + 1..] {
                assert!(key != other_key, "{action:?} and {other:?} share {key}");
            }
        }
    }

    #[test]
    fn every_group_has_actions_and_they_come_together() {
        let groups: Vec<Group> = ACTIONS.iter().map(|info| info.group).collect();
        let mut seen: Vec<Group> = Vec::new();
        for group in groups {
            if seen.last() != Some(&group) {
                assert!(!seen.contains(&group), "{group:?} comes back later");
                seen.push(group);
            }
        }
        assert_eq!(seen.len(), 6);
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
            assert!(Action::named(name).is_some(), "{name}");
        }
    }

    #[test]
    fn every_named_action_has_a_key() {
        for name in named_in_the_windows() {
            let info = Action::named(name).and_then(Action::info).unwrap();
            assert!(!info.keys.is_empty(), "{name} has no key");
        }
    }
}
