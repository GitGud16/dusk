//! What differs between operating systems (CLAUDE.md: Windows first, behind a thin layer):
//! where Dusk keeps its own files, the system's file dialogs, files dropped on the window,
//! which letter a key stands for, and showing a folder in the system's file manager. Linux
//! and macOS add theirs with their builds.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use slint::winit_030::winit::event::{ElementState, WindowEvent};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

#[cfg(windows)]
mod windows;

/// The folder where Dusk keeps the user's settings and shortcuts: `DUSK_SETTINGS_DIR` when set
/// (tests use it), otherwise the user's roaming application data, which follows them from one
/// computer to another (docs/ARCHITECTURE.md, "Keyboard and settings").
pub fn settings_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DUSK_SETTINGS_DIR") {
        return Some(PathBuf::from(dir));
    }
    #[cfg(windows)]
    let dir = std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("Dusk"));
    #[cfg(not(windows))]
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|dir| dir.join("dusk"));
    dir
}

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

/// Shows `dir` in the system's file manager (Explorer on Windows), which runs on its own.
pub fn show_folder(dir: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(dir)
        .spawn()
        .map(drop)
}

/// What a file dialog asks for.
#[derive(Clone, Debug)]
pub enum Dialog {
    /// Media files to import, any number.
    ImportMedia,
    /// A project file to open.
    OpenProject,
    /// A video to compress, one file.
    OpenVideo,
    /// The user's own `ffmpeg` program, for the GPL encoders Dusk does not ship.
    OpenProgram,
    /// A media file the project cannot find, named `name`, which was in `folder`.
    FindMedia {
        name: String,
        folder: Option<PathBuf>,
    },
    /// Where to save the project, starting from the name `suggested`.
    SaveProject { suggested: String },
    /// Where to export a file, starting from `suggested`, a whole path whose extension is
    /// the format's; `kind` names the format for the filter, such as "MP4 video".
    Export { suggested: PathBuf, kind: String },
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

thread_local! {
    /// The character of the key pressed last, until a shortcut takes it.
    static PRESSED: Cell<Option<char>> = const { Cell::new(None) };
}

/// The character of the key whose press Slint is handing on, if it has one: the letter or
/// digit of its virtual-key code, as Windows' own shortcuts take it, or for a punctuation key
/// what it types on a US layout without Shift. That is the letter on the key under Latin
/// keyboard layouts, and the Latin letter at its place under others (Arabic, Cyrillic,
/// Greek), whose letters no shortcut is defined for; the keymap goes by it for punctuation
/// only when the layout types something else there (`keymap::Keys::pressed`).
pub fn pressed_key() -> Option<char> {
    PRESSED.take()
}

/// Brings `window` to the front, out of the taskbar if it was minimized, with the keyboard.
pub fn bring_to_front(window: &slint::Window) {
    window.with_winit_window(|window| {
        window.set_minimized(false);
        window.focus_window();
    });
}

/// Notes every key pressed in `window` for [`pressed_key`] (see [`watch_window`]).
pub fn watch_keys(window: &slint::Window) {
    window.on_winit_window_event(|_, event| {
        note_key(event);
        EventResult::Propagate
    });
}

/// Keeps the character of a key press for [`pressed_key`].
fn note_key(event: &WindowEvent) {
    if let WindowEvent::KeyboardInput { event, .. } = event
        && event.state == ElementState::Pressed
    {
        PRESSED.set(key_character(event.physical_key));
    }
}

/// Calls `dropped` with the files dropped on `window` from another application, all files of
/// one drop at once, and `hovering` with whether files are being dragged over it, and notes
/// every key pressed for [`pressed_key`]. Slint passes on neither, so they are taken from
/// winit's window events, which come before Slint handles them (docs/ARCHITECTURE.md,
/// "Slint specifics").
pub fn watch_window(
    window: &slint::Window,
    dropped: impl Fn(Vec<PathBuf>) + 'static,
    hovering: impl Fn(bool) + 'static,
) {
    let dropped = Rc::new(dropped);
    let waiting: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
    window.on_winit_window_event(move |_, event| {
        note_key(event);
        match event {
            WindowEvent::HoveredFile(_) => hovering(true),
            WindowEvent::HoveredFileCancelled => hovering(false),
            WindowEvent::DroppedFile(path) => {
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

/// The character `key` stands for in the keyboard layout in use, from its virtual-key code
/// (see [`pressed_key`]); `None` for every other key.
fn key_character(key: winit::keyboard::PhysicalKey) -> Option<char> {
    #[cfg(windows)]
    {
        use winit::platform::scancode::PhysicalKeyExtScancode;
        key.to_scancode().and_then(windows::key_character)
    }
    #[cfg(not(windows))]
    {
        let _ = key;
        None
    }
}
