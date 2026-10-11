//! Running a console program from Dusk and feeding its standard input, for the programs that
//! encode exports outside Dusk's own process: a user's own `ffmpeg` (docs/ARCHITECTURE.md,
//! "Optional GPL encoders") and Dusk's helper, `dusq` (docs/ARCHITECTURE.md, "Export").

use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{SendTimeoutError, Sender};

use crate::EngineError;

/// A command that runs `program` without a console window of its own: it is a console
/// program, and Dusk is not.
pub(crate) fn command(program: &Path) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// How often a wait on a program looks at the cancel flag.
pub(crate) const WAIT_STEP: Duration = Duration::from_millis(20);

/// What became of something handed to a [`Feeder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fed {
    /// It is on its way to the program.
    Taken,
    /// The export was cancelled while the program took nothing.
    Cancelled,
    /// The program stopped taking input.
    Stopped,
}

/// Writes what it is fed into a program's input on a thread of its own, so the export never
/// waits on the program longer than it takes to notice a cancel, even when the program stops
/// reading without ending.
pub(crate) struct Feeder<T> {
    items: Option<Sender<T>>,
    thread: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> Feeder<T> {
    /// Starts writing the items it is fed into `input`, each as `write` puts it.
    pub(crate) fn start<W: Write + Send + 'static>(
        mut input: W,
        write: impl Fn(&mut W, &T) -> std::io::Result<()> + Send + 'static,
    ) -> Result<Feeder<T>, EngineError> {
        // One item waits while the one before it is written.
        let (items, waiting) = crossbeam_channel::bounded::<T>(1);
        let thread = thread::Builder::new()
            .name("dusk program input".to_owned())
            .spawn(move || {
                for item in waiting {
                    if write(&mut input, &item).is_err() {
                        // Dropping the receiver tells the export.
                        return;
                    }
                }
                // Dropping the input tells the program the last item came.
            })
            .map_err(EngineError::Thread)?;
        Ok(Feeder {
            items: Some(items),
            thread: Some(thread),
        })
    }

    /// Hands `item` over, waiting while the program is busy unless `cancel` is set.
    pub(crate) fn feed(&self, item: T, cancel: &AtomicBool) -> Fed {
        let Some(items) = &self.items else {
            return Fed::Stopped;
        };
        let mut item = item;
        loop {
            match items.send_timeout(item, WAIT_STEP) {
                Ok(()) => return Fed::Taken,
                Err(SendTimeoutError::Timeout(back)) if !cancel.load(Ordering::Relaxed) => {
                    item = back;
                }
                Err(SendTimeoutError::Timeout(_)) => return Fed::Cancelled,
                Err(SendTimeoutError::Disconnected(_)) => return Fed::Stopped,
            }
        }
    }

    /// Nothing more comes: the thread writes the item waiting, if the program takes it, and
    /// lets go of the input.
    pub(crate) fn close(&mut self) {
        self.items = None;
    }

    /// Closes and waits for the thread, which ends once the program takes the last item or
    /// is gone.
    pub(crate) fn join(&mut self) {
        self.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// Writes an item as its bytes.
    fn bytes<W: Write>() -> impl Fn(&mut W, &Vec<u8>) -> std::io::Result<()> {
        |input, item| input.write_all(item)
    }

    /// A program's input that keeps what it is given.
    #[derive(Clone, Default)]
    struct Kept(Arc<Mutex<Vec<u8>>>);

    impl Write for Kept {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn items_reach_the_program_whole_and_in_order() {
        let kept = Kept::default();
        let mut feeder = Feeder::start(kept.clone(), bytes()).unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(feeder.feed(vec![10; 8], &cancel), Fed::Taken);
        assert_eq!(feeder.feed(vec![20; 4], &cancel), Fed::Taken);
        feeder.join();
        let mut expected = vec![10; 8];
        expected.extend([20; 4]);
        assert_eq!(*kept.0.lock().unwrap(), expected);
    }

    /// A program's input that takes nothing until it is let go.
    struct Stuck(Arc<AtomicBool>);

    impl Write for Stuck {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            while !self.0.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(5));
            }
            Err(std::io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_cancel_is_heard_while_the_program_takes_nothing() {
        let let_go = Arc::new(AtomicBool::new(false));
        let mut feeder = Feeder::start(Stuck(Arc::clone(&let_go)), bytes()).unwrap();
        let cancel = AtomicBool::new(false);
        // One item is being written and one waits; the next cannot be handed over.
        assert_eq!(feeder.feed(vec![1], &cancel), Fed::Taken);
        assert_eq!(feeder.feed(vec![2], &cancel), Fed::Taken);
        cancel.store(true, Ordering::Relaxed);
        let asked = Instant::now();
        assert_eq!(feeder.feed(vec![3], &cancel), Fed::Cancelled);
        assert!(asked.elapsed() < Duration::from_secs(1));
        let_go.store(true, Ordering::Relaxed);
        feeder.join();
    }

    /// A program's input that is closed.
    struct Closed;

    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_program_that_stops_taking_input_is_noticed() {
        let mut feeder = Feeder::start(Closed, bytes()).unwrap();
        let cancel = AtomicBool::new(false);
        let fed: Vec<Fed> = (0..4).map(|n| feeder.feed(vec![n], &cancel)).collect();
        assert!(fed.contains(&Fed::Stopped), "{fed:?}");
        feeder.join();
    }
}
