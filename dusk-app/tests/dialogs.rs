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
