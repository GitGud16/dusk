//! M5's "done when" (docs/ROADMAP.md): an edit made with the keyboard alone, in the real
//! `dusk` binary, so it needs a desktop session and a graphics adapter (hardware, or WARP on
//! CI runners). Keys are posted to the window as Windows delivers typed ones; posted keys
//! cannot hold Shift, Ctrl or Alt, so the edit keeps to keys without them and saves through
//! the question that closing asks.
#![cfg(windows)]

mod common;

use dusk_core::{Frame, TrackKind};

use common::{
    VK_DELETE, VK_F1, VK_HOME, VK_RETURN, VK_RIGHT, VK_TAB, close, press, saved_project_with_clip,
    start, state_dir, type_key, wait_for_clean_exit, wait_for_title, wait_for_window,
};

#[test]
fn an_edit_is_made_and_saved_with_keys_alone() {
    let state = state_dir("keyboard");
    // The sample's 30 frames, picture and sound linked, from frame 0.
    let project = saved_project_with_clip(&state, "keys");
    let mut dusk = start(&[project.as_os_str()], &state);
    let window = wait_for_window(&mut dusk);
    wait_for_title(window, "the project did not open", |title| {
        title == "keys — Dusk"
    });

    // Ten frames in, split everything there, and take the second half away.
    for _ in 0..10 {
        press(window, VK_RIGHT, None);
    }
    type_key(window, 's');
    type_key(window, 'd');
    press(window, VK_DELETE, None);
    // Back to the start: the first half is selected and switched off...
    press(window, VK_HOME, None);
    type_key(window, 'd');
    type_key(window, 'e');
    // ...and starts three frames later.
    for _ in 0..3 {
        press(window, VK_RIGHT, None);
    }
    type_key(window, 'i');
    wait_for_title(window, "the edit left no unsaved changes", |title| {
        title.contains('*')
    });

    // Closing asks about the changes, and Enter saves them.
    close(window);
    press(window, VK_RETURN, None);
    wait_for_clean_exit(&mut dusk);

    let text = std::fs::read_to_string(&project).expect("read the saved project");
    let saved = dusk_core::file::from_json(&text, &project).expect("a project Dusk can open");
    for track in saved.sequence().tracks() {
        let clips: Vec<_> = track
            .clips()
            .iter()
            .map(|clip| (clip.position, clip.length, clip.enabled))
            .collect();
        let expected = match track.kind() {
            _ if track.clips().is_empty() => continue,
            TrackKind::Video => (Frame(3), Frame(7), false),
            TrackKind::Audio => (Frame(3), Frame(7), true),
        };
        assert_eq!(clips, [expected], "{:?} track", track.kind());
    }
    let tracks_with_clips = saved
        .sequence()
        .tracks()
        .iter()
        .filter(|track| !track.clips().is_empty())
        .count();
    assert_eq!(tracks_with_clips, 2);
}

#[test]
fn tab_can_be_an_actions_new_keys() {
    let state = state_dir("keyboard-tab");
    let mut dusk = start(&[], &state);
    let window = wait_for_window(&mut dusk);
    wait_for_title(window, "Dusk did not start", |title| {
        title.ends_with("Dusk")
    });
    // F1 opens the shortcut list on its first action, play or pause; Enter waits for its new
    // keys, and Tab is taken as them rather than moving to the next button.
    press(window, VK_F1, None);
    std::thread::sleep(std::time::Duration::from_millis(500));
    press(window, VK_RETURN, None);
    std::thread::sleep(std::time::Duration::from_millis(300));
    press(window, VK_TAB, None);
    let file = state.join("settings").join("shortcuts.txt");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let taken = loop {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        if text.lines().any(|line| line.trim() == "play-pause = Tab") {
            break true;
        }
        if std::time::Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let _ = dusk.kill();
    assert!(taken, "the shortcuts file does not give play or pause Tab");
}
