//! Dialogs and the question that closing asks, in the real `dusk` binary: the question comes
//! before any dialog, for the eye and for the keys. Needs a desktop session and a graphics
//! adapter (hardware, or WARP on CI runners), as `keyboard.rs` does.
#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};

use dusk_core::{Frame, Project, import};

use common::{
    VK_ESCAPE, VK_RETURN, VK_TAB, close, menu, press, sample, start, state_dir, type_key,
    wait_for_clean_exit, wait_for_title, wait_for_window,
};

/// A project file `<name>.dusk` in `dir` with the sample at frame 0, and a second clip after it
/// whose file is not there.
fn project_with_a_missing_file(dir: &Path, name: &str) -> PathBuf {
    let info = dusk_engine::media_info(&sample()).expect("probe the sample");
    let rate = info.frame_rate.expect("the sample has video");
    let mut project = Project::new(rate, (info.width, info.height));
    import(&project, sample(), info.clone(), Frame(0))
        .apply(&mut project)
        .expect("import the sample");
    let gone = dir.join("gone").join("away.mp4");
    import(&project, gone, info, Frame(30))
        .apply(&mut project)
        .expect("import the missing file");
    let file = dir.join(format!("{name}.dusk"));
    std::fs::write(&file, dusk_core::file::to_json(&project, &file)).expect("save");
    file
}

#[test]
fn the_question_closing_asks_comes_before_the_missing_media_list() {
    let state = state_dir("dialogs-missing");
    let project = project_with_a_missing_file(&state, "missing");
    let mut dusk = start(&[project.as_os_str()], &state);
    let window = wait_for_window(&mut dusk);
    wait_for_title(window, "the project did not open", |title| {
        title == "missing — Dusk"
    });
    // The list of missing files opens by itself; close it, and switch the first clip off.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    press(window, VK_ESCAPE, None);
    type_key(window, 'd');
    type_key(window, 'e');
    wait_for_title(window, "the edit left no unsaved changes", |title| {
        title.contains('*')
    });
    // With the list open again, closing asks about the changes, and Enter saves them.
    menu(window, "Find missing media…");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    close(window);
    press(window, VK_RETURN, None);
    wait_for_clean_exit(&mut dusk);

    let text = std::fs::read_to_string(&project).expect("read the saved project");
    let saved = dusk_core::file::from_json(&text, &project).expect("a project Dusk can open");
    let first = saved.sequence().tracks()[0].clips()[0].clone();
    assert!(!first.enabled, "the edit was saved");
}

#[test]
fn the_missing_list_waits_for_the_question_and_then_opens() {
    // A Dusk that stopped without closing this project left an autosave, so starting on the
    // project asks whether to recover it while the project's missing file is being looked for.
    let state = state_dir("dialogs-waiting");
    let project = project_with_a_missing_file(&state, "waiting");
    let autosave = state.join("autosave");
    std::fs::create_dir_all(&autosave).expect("make the autosave folder");
    std::fs::write(
        autosave.join("left.session"),
        project.to_string_lossy().as_bytes(),
    )
    .expect("write the session record");
    // Written after the project file, as autosaves are, so it holds something newer; a copy
    // would keep the project file's time.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let text = std::fs::read(&project).expect("read the project");
    std::fs::write(autosave.join("left.dusk.autosave"), text).expect("write the autosave");
    let mut dusk = start(&[project.as_os_str()], &state);
    let window = wait_for_window(&mut dusk);
    wait_for_title(window, "the project did not open", |title| {
        title == "waiting — Dusk"
    });
    std::thread::sleep(std::time::Duration::from_millis(1500));
    // Not now: the list that waited behind the question opens, and takes the keys.
    press(window, VK_ESCAPE, None);
    std::thread::sleep(std::time::Duration::from_millis(500));
    type_key(window, 'd');
    type_key(window, 'e');
    std::thread::sleep(std::time::Duration::from_millis(1000));
    let listed = !common::title_of(window).contains('*');
    // Once it is closed, the same keys edit; a slow runner can take a while to say so.
    press(window, VK_ESCAPE, None);
    type_key(window, 'd');
    type_key(window, 'e');
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let edited = loop {
        if common::title_of(window).contains('*') {
            break true;
        }
        if std::time::Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let _ = dusk.kill();
    assert!(
        listed,
        "the keys edited the timeline, so the list did not open"
    );
    assert!(edited, "the keys did not edit once the list was closed");
}

#[test]
fn tab_stays_in_the_question_over_a_dialog() {
    let state = state_dir("dialogs-tab");
    let project = project_with_a_missing_file(&state, "tab");
    let mut dusk = start(&[project.as_os_str()], &state);
    let window = wait_for_window(&mut dusk);
    wait_for_title(window, "the project did not open", |title| {
        title == "tab — Dusk"
    });
    std::thread::sleep(std::time::Duration::from_millis(1500));
    press(window, VK_ESCAPE, None);
    type_key(window, 'd');
    type_key(window, 'e');
    wait_for_title(window, "the edit left no unsaved changes", |title| {
        title.contains('*')
    });
    menu(window, "Find missing media…");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    close(window);
    // Tab goes to the question's first answer, Save, not to the list's Find… behind it,
    // which would ask for a file instead.
    press(window, VK_TAB, None);
    press(window, VK_RETURN, None);
    wait_for_clean_exit(&mut dusk);

    let text = std::fs::read_to_string(&project).expect("read the saved project");
    let saved = dusk_core::file::from_json(&text, &project).expect("a project Dusk can open");
    assert!(
        !saved.sequence().tracks()[0].clips()[0].enabled,
        "the question's Save was pressed"
    );
}
