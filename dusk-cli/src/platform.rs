//! What dusq needs of the system that differs from one system to the next: stopping cleanly
//! on Ctrl+C, so a job removes what it wrote instead of leaving a part file behind.

use std::sync::atomic::AtomicBool;

/// Set when the user presses Ctrl+C or Ctrl+Break: the job stops and cleans up.
pub static STOP: AtomicBool = AtomicBool::new(false);

/// From now on, Ctrl+C sets [`STOP`] instead of ending dusq at once.
#[cfg(windows)]
pub fn stop_on_ctrl_c() {
    use std::sync::atomic::Ordering;

    type Handler = unsafe extern "system" fn(u32) -> i32;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<Handler>, add: i32) -> i32;
    }
    unsafe extern "system" fn handler(event: u32) -> i32 {
        // CTRL_C_EVENT and CTRL_BREAK_EVENT; closing the console still ends dusq.
        if event <= 1 {
            STOP.store(true, Ordering::Relaxed);
            1
        } else {
            0
        }
    }
    // SAFETY: the handler only stores to an atomic, which any thread the system calls it on
    // may do, and it lives as long as the program.
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

/// Other systems come with their builds; until then Ctrl+C ends dusq at once.
#[cfg(not(windows))]
pub fn stop_on_ctrl_c() {}
