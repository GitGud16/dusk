//! The compress tool on the UI thread (docs/ARCHITECTURE.md, "Compress tool paths"): pick a
//! video, read it on a worker, aim it at a size or a quality in one dialog, ask where to save
//! it, and compress it through the engine's one-clip export. The project is never touched.

use std::path::PathBuf;

use dusk_core::MediaInfo;
use dusk_engine::{CompressTarget, EngineError, media_info};
use slint::SharedString;

use crate::CompressView;
use crate::app::{App, file_name, sentence, with_app};
use crate::compress_choices::{CompressChoices, Outcome};
use crate::files::{compress_path, same_file};
use crate::platform::Dialog;

/// The compress dialog while it is open.
pub struct CompressDialog {
    /// The video.
    source: PathBuf,
    /// What is chosen; `None` while the video is being read.
    choices: Option<CompressChoices>,
}

impl App {
    /// Ctrl+M and the File menu: asks for a video, then opens the compress dialog for it.
    pub(crate) fn compress_video(&mut self) {
        if self.export.is_some() {
            return self.say("An export is already running; wait for it to finish or cancel it.");
        }
        if self.compress_dialog.is_some() || self.export_dialog.is_some() {
            return;
        }
        self.show_dialog(Dialog::OpenVideo, |paths| {
            if let Some(path) = paths.into_iter().next() {
                with_app(|app| app.open_compress(path));
            }
        });
    }

    /// Opens the compress dialog for `source`, reading the video on a worker meanwhile.
    fn open_compress(&mut self, source: PathBuf) {
        if self.compress_dialog.is_some() || self.export_dialog.is_some() {
            return;
        }
        self.compress_dialog = Some(CompressDialog {
            source: source.clone(),
            choices: None,
        });
        self.refresh_compress_dialog();
        let spawned = std::thread::Builder::new()
            .name("dusk compress probe".to_owned())
            .spawn(move || {
                let read = media_info(&source).map(|info| {
                    let bytes = std::fs::metadata(&source).map_or(0, |file| file.len());
                    (info, bytes)
                });
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| app.compress_read(source, read));
                });
            });
        if let Err(error) = spawned {
            self.close_compress_dialog();
            self.fail(&EngineError::Thread(error).to_string());
        }
    }

    /// The video at `source` was read.
    fn compress_read(&mut self, source: PathBuf, read: Result<(MediaInfo, u64), EngineError>) {
        // The dialog may have been cancelled meanwhile.
        let Some(dialog) = self
            .compress_dialog
            .as_mut()
            .filter(|dialog| dialog.source == source)
        else {
            return;
        };
        match read {
            Ok((info, bytes)) if info.has_video => {
                dialog.choices = Some(CompressChoices::new(info, bytes));
                self.refresh_compress_dialog();
            }
            Ok(_) => {
                self.close_compress_dialog();
                self.fail(&format!(
                    "{} has no video to compress; pick a video file.",
                    file_name(&source)
                ));
            }
            Err(error) => {
                self.close_compress_dialog();
                self.fail(&sentence(&error.to_string()));
            }
        }
    }

    /// The dialog's `what` changed to `value` (see ui/compress-dialog.slint).
    pub fn compress_changed(&mut self, what: &str, value: i32) {
        let Some(choices) = self
            .compress_dialog
            .as_mut()
            .and_then(|dialog| dialog.choices.as_mut())
        else {
            return;
        };
        match what {
            "by-size" => choices.by_size = value == 0,
            "megabytes" => choices.megabytes = u32::try_from(value).unwrap_or(1).max(1),
            "level" => choices.level = value.clamp(0, 100) as u8,
            "smallest" => {
                if let Outcome::TooSmall { megabytes } = choices.outcome() {
                    choices.megabytes = megabytes;
                }
            }
            _ => {}
        }
        self.refresh_compress_dialog();
    }

    /// The dialog closed: on Compress, ask where and start; on Cancel, nothing.
    pub fn compress_done(&mut self, go: bool) {
        let Some(dialog) = self.compress_dialog.take() else {
            return;
        };
        self.hide_compress_dialog();
        let Some(choices) = dialog.choices.filter(|choices| go && choices.ready()) else {
            return;
        };
        let ask = Dialog::Export {
            suggested: compress_path(&dialog.source),
            kind: "MP4 video".to_owned(),
        };
        let (source, target, info) = (dialog.source, choices.target(), choices.info);
        self.show_dialog(ask, move |paths| {
            if let Some(path) = paths.into_iter().next() {
                with_app(|app| app.start_compress(source, info, path, target));
            }
        });
    }

    /// Compresses `source` into `path`; never over a media file, the video's own included.
    fn start_compress(
        &mut self,
        source: PathBuf,
        info: MediaInfo,
        path: PathBuf,
        target: CompressTarget,
    ) {
        if self.export.is_some() {
            return self.fail("An export is already running; wait for it to finish or cancel it.");
        }
        let over_media = same_file(&source, &path)
            || self
                .project
                .media()
                .iter()
                .any(|media| same_file(&media.path, &path));
        if over_media {
            return self.fail(
                "That file is the video being compressed or used in the project, and Dusk never \
                 writes over media. Choose another name.",
            );
        }
        match self
            .engine
            .compress(source.clone(), info, path.clone(), target)
        {
            Ok(job) => {
                self.export_started(job, false);
                self.compressing = true;
                self.say(&format!(
                    "Compressing {} into {}…",
                    file_name(&source),
                    path.display()
                ));
            }
            Err(error) => self.fail(&sentence(&error.to_string())),
        }
    }

    /// Escape cancels the dialog and Enter compresses once it can; true when the dialog is
    /// open, which then takes every key.
    pub(crate) fn compress_dialog_key(&mut self, text: &str) -> bool {
        use slint::platform::Key;
        let Some(dialog) = &self.compress_dialog else {
            return false;
        };
        let ready = dialog.choices.as_ref().is_some_and(CompressChoices::ready);
        if text == SharedString::from(Key::Escape).as_str() {
            self.compress_done(false);
        } else if text == SharedString::from(Key::Return).as_str() && ready {
            self.compress_done(true);
        }
        true
    }

    fn close_compress_dialog(&mut self) {
        self.compress_dialog = None;
        self.hide_compress_dialog();
    }

    fn hide_compress_dialog(&self) {
        if let Some(window) = self.window() {
            window.set_compress_open(false);
            window.invoke_take_keys();
        }
    }

    /// Shows the dialog as the choices stand.
    fn refresh_compress_dialog(&self) {
        let (Some(dialog), Some(window)) = (&self.compress_dialog, self.window()) else {
            return;
        };
        window.set_compress_view(compress_view(dialog));
        window.set_compress_open(true);
    }
}

/// What the dialog shows of `dialog`.
fn compress_view(dialog: &CompressDialog) -> CompressView {
    let name = file_name(&dialog.source).into();
    let Some(choices) = &dialog.choices else {
        return CompressView {
            reading: true,
            name,
            ..CompressView::default()
        };
    };
    let info = &choices.info;
    let facts = format!(
        "{} long, {} × {}, {}",
        clock(info.duration.0),
        info.width,
        info.height,
        megabytes(choices.file_bytes)
    );
    let (mut outcome, mut warning, smallest) = match choices.outcome() {
        Outcome::Planned {
            width,
            height,
            video,
            sound,
        } => {
            let mut text = format!(
                "Comes out at {width} × {height}, its video at {}",
                rate(video)
            );
            if sound > 0 {
                text += &format!(" and its sound at {}", rate(sound as u64));
            }
            (text + ".", false, 0)
        }
        Outcome::AtQuality { width, height } => (
            format!(
                "Keeps {width} × {height}; how large the file comes out depends on the picture."
            ),
            false,
            0,
        ),
        Outcome::TooSmall { megabytes } => (
            format!("Too small: the smallest this video can become is {megabytes} MB."),
            true,
            i32::try_from(megabytes).unwrap_or(i32::MAX),
        ),
        Outcome::NoLength => (
            "This video does not say how long it is, so it cannot be made to a size; compress \
             it at a quality."
                .to_owned(),
            true,
            0,
        ),
    };
    if choices.not_smaller() {
        outcome += " That is no smaller than the video already is.";
        warning = true;
    }
    CompressView {
        reading: false,
        name,
        facts: facts.into(),
        by_size: choices.by_size,
        megabytes: i32::try_from(choices.megabytes).unwrap_or(i32::MAX),
        level: i32::from(choices.level),
        outcome: outcome.into(),
        warning,
        smallest,
        ready: choices.ready(),
    }
}

/// A length in microseconds as minutes and seconds, or hours too.
fn clock(micros: i64) -> String {
    let seconds = u64::try_from(micros).unwrap_or(0) / 1_000_000;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// A size in bytes as megabytes, one decimal.
pub(crate) fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

/// A bitrate in bits a second as Mbit/s, or kbit/s below one.
fn rate(bits: u64) -> String {
    if bits >= 1_000_000 {
        format!("{:.1} Mbit/s", bits as f64 / 1e6)
    } else {
        format!("{} kbit/s", bits / 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_the_way_people_say_them() {
        assert_eq!(clock(83_000_000), "1:23");
        assert_eq!(clock(3_723_000_000), "1:02:03");
        assert_eq!(megabytes(245_600_000), "245.6 MB");
        assert_eq!(rate(3_105_333), "3.1 Mbit/s");
        assert_eq!(rate(128_000), "128 kbit/s");
    }
}
