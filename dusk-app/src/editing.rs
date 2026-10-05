//! Edits from the UI: importing and placing media, selecting, moving, trimming, splitting and
//! deleting clips, track locks and mutes, clip properties and the sequence settings. Each
//! change is one command on the undo stack (docs/ARCHITECTURE.md, "Commands"); the timeline
//! rules themselves live in `dusk-core`.

use std::path::PathBuf;

use dusk_core::time::{STANDARD_RATES, frame_to_media, media_to_frame};
use dusk_core::{
    ClipEdits, ClipId, Command, Edge, Frame, MediaId, MediaInfo, MediaKind, Project, Rational,
    RemoveClips, SetAudioEdits, SetClipEnabled, SetSequenceSettings, SetTrackLocked, SetTrackMuted,
    SetVideoEdits, TrackId, TrackKind, TrimClips, Unlink, add_media, nearest_free_place,
    nearest_free_position, place, place_where_free, remove_one, split_at,
};
use dusk_engine::{EngineError, media_info};

use crate::app::{App, absolute, file_name, frame_int, sentence, with_app};
use crate::document::Question;
use crate::platform::Dialog;
use crate::timeline::{self, tracks_for_row};

impl App {
    /// Asks which files to import, then imports them into the bin.
    pub fn import_dialog(&mut self) {
        self.show_dialog(Dialog::ImportMedia, |paths| {
            if !paths.is_empty() {
                with_app(|app| app.import_files(paths, false));
            }
        });
    }

    /// Files were dropped on the window: a project file is opened, anything else imported.
    pub fn import_dropped(&mut self, paths: Vec<PathBuf>) {
        let project = paths.iter().find(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("dusk"))
        });
        match project {
            Some(project) => {
                let project = project.clone();
                self.open_project_file(project);
            }
            None => self.import_files(paths, false),
        }
    }

    /// Reads what each of `paths` holds off the UI thread, then adds them to the project as
    /// one undoable step; with `place`, also lays them on the timeline one after another.
    pub fn import_files(&mut self, paths: Vec<PathBuf>, place: bool) {
        let paths = absolute(paths);
        let what = match paths.as_slice() {
            [one] => file_name(one),
            many => format!("{} files", many.len()),
        };
        self.say(&format!("Importing {what}…"));
        let spawned = std::thread::Builder::new()
            .name("dusk import".to_owned())
            .spawn(move || {
                let probed: Vec<_> = paths
                    .into_iter()
                    .map(|path| {
                        let info = media_info(&path);
                        (path, info)
                    })
                    .collect();
                let _ = slint::invoke_from_event_loop(move || {
                    with_app(|app| app.imported(probed, place));
                });
            });
        if let Err(error) = spawned {
            self.fail(&EngineError::Thread(error).to_string());
        }
    }

    fn imported(&mut self, probed: Vec<(PathBuf, Result<MediaInfo, EngineError>)>, place: bool) {
        let total = probed.len();
        let mut scratch = Project::clone(&self.project);
        let mut commands = Vec::new();
        let mut added = Vec::new();
        let mut problems = Vec::new();
        for (path, info) in probed {
            let name = file_name(&path);
            let info = match info {
                Ok(info) => info,
                Err(error) => {
                    problems.push(format!("{name} ({error})"));
                    continue;
                }
            };
            if scratch.media().iter().any(|media| media.path == path) {
                problems.push(format!("{name} (it is in the project already)"));
                continue;
            }
            let (id, mut add) = add_media(&scratch, path, info);
            match add.apply(&mut scratch) {
                Ok(()) => {
                    commands.push(add);
                    added.push(id);
                }
                Err(rejection) => problems.push(format!("{name} ({rejection})")),
            }
        }
        if place {
            for &media in &added {
                // On an empty timeline the first video sets the sequence, as in M1.
                if let Some((rate, size)) = settings_to_match(&scratch, media) {
                    let mut settings =
                        Command::SetSequenceSettings(SetSequenceSettings::new(rate, size));
                    if settings.apply(&mut scratch).is_ok() {
                        commands.push(settings);
                    }
                }
                let end = scratch.sequence().end();
                if let Ok(mut placed) = place_where_free(&scratch, media, end)
                    && placed.apply(&mut scratch).is_ok()
                {
                    commands.push(placed);
                }
            }
        }
        if !commands.is_empty() && self.edit(Command::Batch(commands)) {
            self.selected_media = added.first().copied();
            self.refresh_bin();
        }
        let imported = added.len();
        match (imported, problems.as_slice()) {
            (_, []) if imported == 1 => self.say("Imported 1 file."),
            (_, []) => self.say(&format!("Imported {imported} files.")),
            (0, [one]) => self.fail(&format!("Could not import {one}.")),
            _ => self.fail(&format!(
                "Imported {imported} of {total} files. Could not import {}.",
                problems.join(", ")
            )),
        }
    }

    pub fn select_clip(&mut self, clip: i32) {
        self.selected_clip = u64::try_from(clip).ok().map(ClipId);
        self.refresh_selection();
        self.refresh_properties();
    }

    pub fn select_media(&mut self, media: i32) {
        self.selected_media = u64::try_from(media).ok().map(MediaId);
        self.refresh_bin();
    }

    /// Media from the bin was dropped on `row` at `frame`, a place where it fits.
    pub fn place_media(&mut self, media: i32, row: i32, frame: i32) {
        let (Ok(media), Ok(row)) = (u64::try_from(media), usize::try_from(row)) else {
            return;
        };
        self.place(MediaId(media), Some(row), Frame(frame.into()));
    }

    /// A media item was double-clicked: placed at the playhead.
    pub fn place_media_at_playhead(&mut self, media: i32) {
        if let Ok(media) = u64::try_from(media) {
            self.place(MediaId(media), None, self.playhead);
        }
    }

    pub(crate) fn place_selected_at_playhead(&mut self) {
        match self.selected_media {
            Some(media) => self.place(media, None, self.playhead),
            None => self.say("Select media in the bin to place it."),
        }
    }

    /// Places `media` at `at`: on the tracks of `row`, or on the first pair of tracks with
    /// room. The first video asks whether the sequence should match it.
    fn place(&mut self, media: MediaId, row: Option<usize>, at: Frame) {
        if let Some((rate, size)) = settings_to_match(&self.project, media) {
            let sequence = self.project.sequence();
            let (width, height) = sequence.resolution();
            let name = self
                .project
                .media_ref(media)
                .map_or_else(String::new, |media| file_name(&media.path));
            let message = format!(
                "{name} is {}×{} at {} fps; the sequence is {width}×{height} at {} fps. Matching \
                 makes it fill the frame without bars. You can change this later in Sequence \
                 settings.",
                size.0,
                size.1,
                rate_name(rate),
                rate_name(sequence.frame_rate()),
            );
            let question = Question::MatchSequence {
                media,
                row,
                at,
                rate,
                size,
            };
            self.ask(
                question,
                "Match the sequence to this clip?",
                &message,
                &["Match the sequence", "Keep the sequence", "Cancel"],
                None,
            );
            return;
        }
        self.place_now(media, row, at, None);
    }

    /// Places `media`, first giving the sequence `settings` when there are some; one step.
    pub(crate) fn place_now(
        &mut self,
        media: MediaId,
        row: Option<usize>,
        mut at: Frame,
        settings: Option<(Rational, (u32, u32))>,
    ) {
        let mut scratch = Project::clone(&self.project);
        let mut commands = Vec::new();
        if let Some((rate, size)) = settings {
            let old_rate = scratch.sequence().frame_rate();
            let mut change = Command::SetSequenceSettings(SetSequenceSettings::new(rate, size));
            if let Err(rejection) = change.apply(&mut scratch) {
                return self.fail(&sentence(&rejection.to_string()));
            }
            commands.push(change);
            // The drop point, counted at the old rate, at the new one.
            at = media_to_frame(frame_to_media(at, old_rate), rate);
        }
        let new_clip = scratch.fresh_ids().clip();
        let placed = match row {
            Some(row) => match tracks_for_row(&scratch, row) {
                Some((video, audio)) => {
                    let at = nearest_free_place(&scratch, media, at, video, audio).unwrap_or(at);
                    place(&scratch, media, at, video, audio)
                }
                None => return,
            },
            None => place_where_free(&scratch, media, at),
        };
        match placed {
            Ok(command) => commands.push(command),
            Err(rejection) => return self.fail(&sentence(&rejection.to_string())),
        }
        let command = match commands.len() {
            1 => commands.remove(0),
            _ => Command::Batch(commands),
        };
        if self.edit(command) {
            self.selected_clip = Some(new_clip);
            self.refresh_selection();
            self.refresh_properties();
        }
    }

    /// Where the media dragged from the bin would start if dropped on `row` at `frame`; -1
    /// where it cannot go.
    pub fn snap_place(&self, media: i32, row: i32, frame: i32) -> i32 {
        let (Ok(media), Ok(row)) = (u64::try_from(media), usize::try_from(row)) else {
            return -1;
        };
        let Some((video, audio)) = tracks_for_row(&self.project, row) else {
            return -1;
        };
        nearest_free_place(
            &self.project,
            MediaId(media),
            Frame(frame.into()),
            video,
            audio,
        )
        .map_or(-1, frame_int)
    }

    /// Where `clip` would start if dropped on `row` at `frame`: the nearest free frame on its
    /// tracks, or -1 where it cannot go.
    pub fn snap_move(&self, clip: i32, frame: i32, row: i32) -> i32 {
        let Ok(clip) = u64::try_from(clip).map(ClipId) else {
            return -1;
        };
        let track = self.moved_to_track(clip, row);
        nearest_free_position(&self.project, clip, Frame(frame.into()), track).map_or(-1, frame_int)
    }

    /// A clip was dragged to `row` at `frame`.
    pub fn move_clip(&mut self, clip: i32, frame: i32, row: i32) {
        let Ok(clip) = u64::try_from(clip).map(ClipId) else {
            return;
        };
        let track = self.moved_to_track(clip, row);
        self.edit(Command::MoveClips(dusk_core::MoveClips::new(
            clip,
            Frame(frame.into()),
            track,
        )));
    }

    /// The track `clip` changes to when dragged onto `row`; `None` when it stays on its own.
    fn moved_to_track(&self, clip: ClipId, row: i32) -> Option<TrackId> {
        let rows = timeline::track_rows(&self.project);
        let target = rows.get(usize::try_from(row).ok()?)?.id;
        let (current, _) = self.project.find_clip(clip)?;
        (current.id() != target).then_some(target)
    }

    /// A trim handle of `clip` was dragged to `frame`.
    pub fn trim(&mut self, clip: i32, start: bool, frame: i32) {
        let Ok(clip) = u64::try_from(clip).map(ClipId) else {
            return;
        };
        let edge = if start { Edge::Start } else { Edge::End };
        self.edit(Command::TrimClips(TrimClips::new(
            clip,
            edge,
            Frame(frame.into()),
        )));
    }

    /// S: splits the selected clip at the playhead when it is there, otherwise every clip
    /// under the playhead.
    pub(crate) fn split(&mut self) {
        let at = self.playhead;
        let selected = self.selected_clip.filter(|clip| {
            self.project
                .find_clip(*clip)
                .is_some_and(|(_, clip)| clip.position < at && at < clip.end())
        });
        match split_at(&self.project, at, selected) {
            Ok(command) => {
                self.edit(command);
            }
            Err(rejection) => self.fail(&sentence(&rejection.to_string())),
        }
    }

    /// Deletes the selected clip and its linked clips, closing the gap with `ripple`.
    pub(crate) fn delete(&mut self, ripple: bool) {
        let Some(clip) = self.selected_clip else {
            return self.say("Select a clip to delete it.");
        };
        if self.edit(Command::RemoveClips(RemoveClips::new(clip, ripple))) {
            self.selected_clip = None;
            self.refresh_properties();
        }
    }

    /// Alt+Delete: deletes only the selected clip; its partners stay, unlinked.
    pub(crate) fn delete_one(&mut self) {
        let Some(clip) = self.selected_clip else {
            return self.say("Select a clip to delete it.");
        };
        let command = remove_one(&self.project, clip);
        if self.edit(command) {
            self.selected_clip = None;
            self.refresh_properties();
        }
    }

    /// Detaches the selected clip's video and audio, so each moves on its own.
    pub fn unlink(&mut self) {
        let Some(clip) = self.selected_clip else {
            return self.say("Select a linked clip to detach its audio.");
        };
        if self.edit(Command::Unlink(Unlink::new(clip))) {
            self.say("Detached: the video and the audio now move on their own.");
        }
    }

    /// E: enables or disables the selected clip.
    pub(crate) fn toggle_enabled(&mut self) {
        let Some((_, clip)) = self
            .selected_clip
            .and_then(|clip| self.project.find_clip(clip))
        else {
            return self.say("Select a clip to enable or disable it.");
        };
        let (id, enabled) = (clip.id, !clip.enabled);
        self.edit(Command::SetClipEnabled(SetClipEnabled::new(id, enabled)));
    }

    /// Locks (`lock`) or mutes the track at `index` in the sequence, or undoes that.
    pub(crate) fn toggle_track(&mut self, index: usize, lock: bool) {
        let Some(track) = self.project.sequence().tracks().get(index) else {
            return;
        };
        let id = track.id();
        self.edit(if lock {
            Command::SetTrackLocked(SetTrackLocked::new(id, !track.locked()))
        } else {
            Command::SetTrackMuted(SetTrackMuted::new(id, !track.muted()))
        });
    }

    /// A track header's lock or mute toggle was clicked.
    pub fn toggle_track_by_id(&mut self, track: i32, lock: bool) {
        let index = self
            .project
            .sequence()
            .tracks()
            .iter()
            .position(|candidate| i64::try_from(candidate.id().0) == Ok(track.into()));
        if let Some(index) = index {
            self.toggle_track(index, lock);
        }
    }

    pub fn set_clip_enabled(&mut self, enabled: bool) {
        if let Some(clip) = self.selected_clip {
            self.edit(Command::SetClipEnabled(SetClipEnabled::new(clip, enabled)));
        }
    }

    /// Fits the selected clip into the frame with bars, or fills it, cropping the rest.
    pub fn set_clip_fill(&mut self, fill: bool) {
        let Some((_, clip)) = self
            .selected_clip
            .and_then(|clip| self.project.find_clip(clip))
        else {
            return;
        };
        let ClipEdits::Video(edits) = &clip.edits else {
            return;
        };
        let fit = if fill {
            dusk_core::Fit::Fill
        } else {
            dusk_core::Fit::Fit
        };
        if edits.fit == fit {
            return;
        }
        let edits = dusk_core::VideoEdits {
            fit,
            ..edits.clone()
        };
        let id = clip.id;
        self.edit(Command::SetVideoEdits(SetVideoEdits::new(id, edits)));
    }

    /// F: fits the selected clip's picture with bars, or fills the frame with it.
    pub(crate) fn toggle_fill(&mut self) {
        let fill = self
            .selected_clip
            .and_then(|clip| self.project.find_clip(clip))
            .and_then(|(_, clip)| match &clip.edits {
                ClipEdits::Video(edits) => Some(edits.fit != dusk_core::Fit::Fill),
                ClipEdits::Audio(_) => None,
            });
        match fill {
            Some(fill) => self.set_clip_fill(fill),
            None => self.say("Select a video clip to fit or fill its picture."),
        }
    }

    /// Gives the selected photo a length of `frames`.
    pub fn set_clip_length(&mut self, frames: i32) {
        let Some((_, clip)) = self
            .selected_clip
            .and_then(|clip| self.project.find_clip(clip))
        else {
            return;
        };
        let (id, end) = (clip.id, clip.position + Frame(frames.into()));
        self.edit(Command::TrimClips(TrimClips::new(id, Edge::End, end)));
    }

    pub fn set_clip_volume(&mut self, decibels: f32) {
        self.change_audio(|edits| edits.volume_db = decibels);
    }

    pub fn set_clip_fades(&mut self, fade_in: i32, fade_out: i32) {
        self.change_audio(|edits| {
            edits.fade_in = Frame(fade_in.into());
            edits.fade_out = Frame(fade_out.into());
        });
    }

    /// Changes the selected audio clip's edits with `change`, as one step if anything changed.
    fn change_audio(&mut self, change: impl FnOnce(&mut dusk_core::AudioEdits)) {
        let Some((_, clip)) = self
            .selected_clip
            .and_then(|clip| self.project.find_clip(clip))
        else {
            return;
        };
        let ClipEdits::Audio(before) = &clip.edits else {
            return;
        };
        let mut edits = before.clone();
        change(&mut edits);
        if edits == *before {
            return;
        }
        let id = clip.id;
        self.edit(Command::SetAudioEdits(SetAudioEdits::new(id, edits)));
        // A refused value goes back to what the clip has.
        self.refresh_properties();
    }

    /// Shows the sequence settings dialog.
    pub(crate) fn open_sequence_settings(&mut self) {
        let Some(window) = self.window() else {
            return;
        };
        let sequence = self.project.sequence();
        let rates = rate_choices(sequence.frame_rate());
        let names: Vec<slint::SharedString> =
            rates.iter().map(|rate| rate_name(*rate).into()).collect();
        let current = rates
            .iter()
            .position(|rate| *rate == sequence.frame_rate())
            .unwrap_or(0);
        let (width, height) = sequence.resolution();
        window.set_rate_names(std::rc::Rc::new(slint::VecModel::from(names)).into());
        window.set_sequence_rate(current as i32);
        window.set_sequence_width(i32::try_from(width).unwrap_or(i32::MAX));
        window.set_sequence_height(i32::try_from(height).unwrap_or(i32::MAX));
        window.set_sequence_settings_open(true);
        self.sequence_settings_open = true;
    }

    pub(crate) fn close_sequence_settings(&mut self) {
        self.sequence_settings_open = false;
        if let Some(window) = self.window() {
            window.set_sequence_settings_open(false);
        }
    }

    /// The sequence settings dialog closed: applied with these choices, or cancelled.
    pub fn sequence_settings_done(&mut self, apply: bool, rate: i32, width: i32, height: i32) {
        self.close_sequence_settings();
        if !apply {
            return;
        }
        let sequence = self.project.sequence();
        let rates = rate_choices(sequence.frame_rate());
        let (Some(rate), Ok(width), Ok(height)) = (
            usize::try_from(rate)
                .ok()
                .and_then(|index| rates.get(index))
                .copied(),
            u32::try_from(width),
            u32::try_from(height),
        ) else {
            return;
        };
        if (rate, (width, height)) == (sequence.frame_rate(), sequence.resolution()) {
            return;
        }
        let change = SetSequenceSettings::new(rate, (width, height));
        if self.edit(Command::SetSequenceSettings(change)) {
            // Every clip moved to new frame numbers.
            self.view.fit();
            self.refresh_timeline();
        }
    }
}

/// The rate and size the sequence would take from `media`: only for the first video placed
/// (stills and sound leave the sequence as it is), and only when they differ.
fn settings_to_match(project: &Project, media: MediaId) -> Option<(Rational, (u32, u32))> {
    let info = &project.media_ref(media)?.info;
    if info.kind != MediaKind::Video || !info.has_video {
        return None;
    }
    let has_video = project.sequence().tracks().iter().any(|track| {
        track.kind() == TrackKind::Video
            && track.clips().iter().any(|clip| {
                project
                    .media_ref(clip.media_id)
                    .is_some_and(|media| media.info.kind == MediaKind::Video)
            })
    });
    let wanted = (info.frame_rate?, (info.width, info.height));
    let sequence = project.sequence();
    let current = (sequence.frame_rate(), sequence.resolution());
    (!has_video && wanted != current).then_some(wanted)
}

/// The rates the sequence settings offer: the standard ones, and `current` if it is not one.
fn rate_choices(current: Rational) -> Vec<Rational> {
    let mut rates = STANDARD_RATES.to_vec();
    if !rates.contains(&current) {
        rates.push(current);
    }
    rates
}

/// A rate as people write it: "30", "29.97", "23.976".
fn rate_name(rate: Rational) -> String {
    let fps = f64::from(rate.num()) / f64::from(rate.den());
    let text = format!("{fps:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(num: u32, den: u32) -> Rational {
        Rational::new(num, den).unwrap()
    }

    #[test]
    fn rates_read_as_people_write_them() {
        let names: Vec<String> = STANDARD_RATES.iter().map(|rate| rate_name(*rate)).collect();
        assert_eq!(
            names,
            ["23.976", "24", "25", "29.97", "30", "50", "59.94", "60"]
        );
        assert_eq!(rate_name(rate(120, 1)), "120");
    }

    #[test]
    fn an_unusual_rate_is_offered_beside_the_standard_ones() {
        assert_eq!(rate_choices(rate(30, 1)).len(), 8);
        let choices = rate_choices(rate(120, 1));
        assert_eq!(choices.len(), 9);
        assert_eq!(choices[8], rate(120, 1));
    }

    fn info(kind: MediaKind, rate_and_size: Option<(Rational, (u32, u32))>) -> MediaInfo {
        let (frame_rate, (width, height)) = match rate_and_size {
            Some((rate, size)) => (Some(rate), size),
            None => (None, (0, 0)),
        };
        MediaInfo {
            kind,
            duration: dusk_core::MediaTime(2_000_000),
            has_video: kind != MediaKind::Audio,
            has_audio: kind != MediaKind::Still,
            frame_rate,
            vfr: false,
            width,
            height,
            orientation: dusk_core::Orientation::UPRIGHT,
        }
    }

    fn with_media(project: &mut Project, info: MediaInfo) -> MediaId {
        let (id, mut add) = add_media(project, "x".into(), info);
        add.apply(project).unwrap();
        id
    }

    #[test]
    fn only_the_first_video_asks_to_match_the_sequence() {
        let mut project = Project::new(rate(30, 1), (1920, 1080));
        let portrait = Some((rate(30000, 1001), (1080, 1920)));
        let phone = with_media(&mut project, info(MediaKind::Video, portrait));
        let song = with_media(&mut project, info(MediaKind::Audio, None));
        let photo = with_media(
            &mut project,
            info(MediaKind::Still, Some((rate(30, 1), (4032, 3024)))),
        );
        assert_eq!(settings_to_match(&project, phone), portrait);
        assert_eq!(settings_to_match(&project, song), None);
        assert_eq!(settings_to_match(&project, photo), None);
        // A photo on the timeline does not count as the first video.
        place_where_free(&project, photo, Frame(0))
            .unwrap()
            .apply(&mut project)
            .unwrap();
        assert_eq!(settings_to_match(&project, phone), portrait);
        place_where_free(&project, phone, Frame(200))
            .unwrap()
            .apply(&mut project)
            .unwrap();
        let other = with_media(&mut project, info(MediaKind::Video, portrait));
        assert_eq!(settings_to_match(&project, other), None);
    }

    #[test]
    fn a_video_that_matches_already_asks_nothing() {
        let mut project = Project::new(rate(30, 1), (1920, 1080));
        let same = with_media(
            &mut project,
            info(MediaKind::Video, Some((rate(30, 1), (1920, 1080)))),
        );
        assert_eq!(settings_to_match(&project, same), None);
    }
}
