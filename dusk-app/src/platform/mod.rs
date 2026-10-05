//! What differs between operating systems (CLAUDE.md: Windows first, behind a thin layer):
//! where Dusk keeps its own files, the system's file dialogs, and files dropped on the
//! window. Linux and macOS add theirs with their builds.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

#[cfg(windows)]
mod windows;

/// The folder where Dusk keeps its own files, such as autosaves: `DUSK_STATE_DIR` when set
/// (tests use it), otherwise the user's local application data.
pub fn state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DUSK_STATE_DIR") {
        return Some(PathBuf::from(dir));
    }
    #[cfg(windows)]
    let dir = std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join("Dusk"));
    #[cfg(not(windows))]
    let dir = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .map(|dir| dir.join("dusk"));
    dir
}

/// What a file dialog asks for.
#[derive(Clone, Debug)]
pub enum Dialog {
    /// Media files to import, any number.
    ImportMedia,
    /// A project file to open.
    OpenProject,
    /// Where to save the project, starting from the name `suggested`.
    SaveProject { suggested: String },
}

/// Shows the system's file dialog for `dialog` over `window`, on a thread of its own so the
/// editor keeps drawing, and calls `done` on the UI thread with what was picked: nothing when
/// the dialog was cancelled.
pub fn show_dialog(
    window: &slint::Window,
    dialog: Dialog,
    done: impl FnOnce(Vec<PathBuf>) + Send + 'static,
) {
    #[cfg(windows)]
    {
        let owner = window_handle(window);
        let spawned = std::thread::Builder::new()
            .name("dusk file dialog".to_owned())
            .spawn(move || {
                let picked = windows::show(owner, &dialog);
                let _ = slint::invoke_from_event_loop(move || done(picked));
            });
        if spawned.is_err() {
            // Without a thread there is no dialog; the editor carries on as if cancelled.
            let _ = slint::invoke_from_event_loop(move || {});
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (window, dialog);
        let _ = slint::invoke_from_event_loop(move || done(Vec::new()));
    }
}

/// The native handle of `window`, for owning a dialog.
#[cfg(windows)]
fn window_handle(window: &slint::Window) -> Option<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    window
        .with_winit_window(|window| match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
            _ => None,
        })
        .flatten()
}

/// Calls `dropped` with the files dropped on `window` from another application, all files of
/// one drop at once, and `hovering` with whether files are being dragged over it. Slint does
/// not pass these on, so they are taken from winit's window events (docs/ARCHITECTURE.md,
/// "Slint specifics").
pub fn on_files_dropped(
    window: &slint::Window,
    dropped: impl Fn(Vec<PathBuf>) + 'static,
    hovering: impl Fn(bool) + 'static,
) {
    let dropped = Rc::new(dropped);
    let waiting: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
    window.on_winit_window_event(move |_, event| {
        match event {
            winit::event::WindowEvent::HoveredFile(_) => hovering(true),
            winit::event::WindowEvent::HoveredFileCancelled => hovering(false),
            winit::event::WindowEvent::DroppedFile(path) => {
                hovering(false);
                let first = waiting.borrow().is_empty();
                waiting.borrow_mut().push(path.clone());
                if first {
                    // The drop's other files arrive before the timer fires.
                    let (waiting, dropped) = (Rc::clone(&waiting), Rc::clone(&dropped));
                    slint::Timer::single_shot(Duration::ZERO, move || dropped(waiting.take()));
                }
            }
            _ => {}
        }
        EventResult::Propagate
    });
}
