//! The undo stack (docs/ARCHITECTURE.md, "Commands"): the commands applied so far and a
//! cursor; undo reverts the command before the cursor, redo applies the one after it again.

use dusk_core::{Command, Project, Rejection};

/// Applied commands, in order, and how many of them are in effect.
#[derive(Default)]
pub struct History {
    /// Each command with the state it leads to.
    commands: Vec<(Command, u64)>,
    cursor: usize,
    /// The last state handed out.
    last_state: u64,
}

impl History {
    /// Applies `command` to `project` and keeps it for undo, dropping anything undone before;
    /// a refused command leaves both unchanged.
    pub fn apply(
        &mut self,
        mut command: Command,
        project: &mut Project,
    ) -> Result<&Command, Rejection> {
        command.apply(project)?;
        self.commands.truncate(self.cursor);
        self.last_state += 1;
        self.commands.push((command, self.last_state));
        self.cursor = self.commands.len();
        Ok(&self.commands[self.cursor - 1].0)
    }

    /// Reverts the last command in effect; false when there is none.
    pub fn undo(&mut self, project: &mut Project) -> bool {
        let Some(last) = self.cursor.checked_sub(1) else {
            return false;
        };
        self.commands[last].0.revert(project);
        self.cursor = last;
        true
    }

    /// Applies the last undone command again; false when there is none.
    pub fn redo(&mut self, project: &mut Project) -> Result<bool, Rejection> {
        let Some((command, _)) = self.commands.get_mut(self.cursor) else {
            return Ok(false);
        };
        command.apply(project)?;
        self.cursor += 1;
        Ok(true)
    }

    /// Names the project's state: equal only for the same commands in effect, so a saved
    /// state can be recognized after undo and redo.
    pub fn state(&self) -> u64 {
        self.cursor
            .checked_sub(1)
            .map_or(0, |last| self.commands[last].1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::file::{from_json, to_json};
    use dusk_core::{
        AudioEdits, ClipId, Edge, Frame, MediaInfo, MediaKind, MediaTime, Rational, SetAudioEdits,
        SetClipEnabled, TrackKind, TrimClips, add_media, import, place_where_free, split_at,
    };

    fn project() -> Project {
        Project::new(Rational::new(30, 1).unwrap(), (1920, 1080))
    }

    fn clip_info() -> MediaInfo {
        MediaInfo {
            kind: MediaKind::Video,
            duration: MediaTime(2_000_000),
            has_video: true,
            has_audio: true,
            frame_rate: Rational::new(30, 1),
            vfr: false,
            width: 1920,
            height: 1080,
            orientation: dusk_core::Orientation::UPRIGHT,
        }
    }

    fn end_of(project: &Project) -> Frame {
        project.sequence().end()
    }

    fn trim_end(project: &Project, to: i64) -> Command {
        let clip = project.sequence().tracks()[0].clips()[0].id;
        Command::TrimClips(TrimClips::new(clip, Edge::End, Frame(to)))
    }

    fn imported() -> (Project, History) {
        let mut project = project();
        let mut history = History::default();
        let import = import(&project, "a.mp4".into(), clip_info(), Frame(0));
        history.apply(import, &mut project).unwrap();
        (project, history)
    }

    /// The clip on the first track of `kind` that covers `frame`.
    fn clip_at(project: &Project, kind: TrackKind, frame: i64) -> ClipId {
        let track = project
            .sequence()
            .tracks()
            .iter()
            .find(|track| track.kind() == kind)
            .unwrap();
        let clip = track
            .clips()
            .iter()
            .find(|clip| clip.position <= Frame(frame) && Frame(frame) < clip.end());
        clip.unwrap().id
    }

    /// M2 is done when a three-clip edit with music survives save, quit and reopen, and undo
    /// history behaves (docs/ROADMAP.md): saved and read back, the edit is the same project;
    /// undo takes it apart one step at a time, through every state it went through, and redo
    /// puts it together again; reopened, it starts a history of its own.
    #[test]
    fn a_three_clip_edit_with_music_survives_save_and_reopen() {
        let folder = std::env::temp_dir().join("dusk-edit");
        let mut project = project();
        let mut history = History::default();
        let mut states = vec![project.clone()];
        let mut edit = |command: Command, project: &mut Project| {
            history.apply(command, project).unwrap();
            states.push(project.clone());
        };
        // Three clips with sound, one after another.
        for (name, at) in [("a.mp4", 0), ("b.mp4", 60), ("c.mp4", 120)] {
            let command = import(&project, folder.join(name), clip_info(), Frame(at));
            edit(command, &mut project);
        }
        // Music under them, on the second pair of tracks.
        let music = MediaInfo {
            kind: MediaKind::Audio,
            duration: MediaTime(10_000_000),
            has_video: false,
            frame_rate: None,
            width: 0,
            height: 0,
            ..clip_info()
        };
        let (song, add) = add_media(&project, folder.join("song.mp3"), music);
        edit(add, &mut project);
        let place = place_where_free(&project, song, Frame(0)).unwrap();
        edit(place, &mut project);
        // Cut the middle clip, shorten the last, quiet the music and fade it out, and hide
        // the first clip's picture.
        edit(split_at(&project, Frame(90), None).unwrap(), &mut project);
        let last = clip_at(&project, TrackKind::Video, 150);
        edit(
            Command::TrimClips(TrimClips::new(last, Edge::End, Frame(165))),
            &mut project,
        );
        let song_clip = project.sequence().tracks()[3].clips()[0].id;
        let quieter = AudioEdits {
            volume_db: -6.0,
            fade_in: Frame(0),
            fade_out: Frame(30),
        };
        edit(
            Command::SetAudioEdits(SetAudioEdits::new(song_clip, quieter)),
            &mut project,
        );
        let first = clip_at(&project, TrackKind::Video, 10);
        edit(
            Command::SetClipEnabled(SetClipEnabled::new(first, false)),
            &mut project,
        );

        // Saved, the app quits; opened again, the file holds the same edit.
        let file = folder.join("edit.dusk");
        let reopened = from_json(&to_json(&project, &file), &file).unwrap();
        assert_eq!(reopened, project);

        // Undo walks back through every state, redo forward again.
        for state in states.iter().rev().skip(1) {
            assert!(history.undo(&mut project));
            assert_eq!(&project, state);
        }
        assert!(!history.undo(&mut project));
        for state in states.iter().skip(1) {
            assert!(history.redo(&mut project).unwrap());
            assert_eq!(&project, state);
        }
        assert_eq!(project, reopened);

        // The reopened project has nothing to undo, and its own edits undo back to it.
        let mut reopened_project = reopened.clone();
        let mut fresh = History::default();
        assert!(!fresh.undo(&mut reopened_project));
        let trim = Command::TrimClips(TrimClips::new(
            clip_at(&reopened_project, TrackKind::Video, 70),
            Edge::End,
            Frame(80),
        ));
        fresh.apply(trim, &mut reopened_project).unwrap();
        assert_ne!(reopened_project, reopened);
        assert!(fresh.undo(&mut reopened_project));
        assert_eq!(reopened_project, reopened);
    }

    #[test]
    fn undo_and_redo_step_through_the_commands() {
        let (mut project, mut history) = imported();
        history.apply(trim_end(&project, 30), &mut project).unwrap();
        assert_eq!(end_of(&project), Frame(30));

        assert!(history.undo(&mut project));
        assert_eq!(end_of(&project), Frame(60));
        assert!(history.undo(&mut project));
        assert_eq!(end_of(&project), Frame(0));
        assert!(!history.undo(&mut project));

        assert!(history.redo(&mut project).unwrap());
        assert!(history.redo(&mut project).unwrap());
        assert_eq!(end_of(&project), Frame(30));
        assert!(!history.redo(&mut project).unwrap());
    }

    #[test]
    fn a_new_command_drops_what_was_undone() {
        let (mut project, mut history) = imported();
        history.apply(trim_end(&project, 30), &mut project).unwrap();
        history.undo(&mut project);
        history.apply(trim_end(&project, 45), &mut project).unwrap();
        assert!(!history.redo(&mut project).unwrap());
        assert_eq!(end_of(&project), Frame(45));
    }

    #[test]
    fn the_state_says_whether_the_project_differs_from_a_saved_one() {
        let (mut project, mut history) = imported();
        let saved = history.state();
        history.apply(trim_end(&project, 30), &mut project).unwrap();
        assert_ne!(history.state(), saved);
        history.undo(&mut project);
        assert_eq!(history.state(), saved);
        // Another edit after the undo is another state, though as many commands are in effect.
        history.apply(trim_end(&project, 45), &mut project).unwrap();
        let other = history.state();
        assert_ne!(other, saved);
        history.undo(&mut project);
        history.redo(&mut project).unwrap();
        assert_eq!(history.state(), other);
        history.undo(&mut project);
        history.undo(&mut project);
        assert_ne!(history.state(), saved);
        assert_eq!(history.state(), History::default().state());
    }

    #[test]
    fn a_refused_command_changes_nothing() {
        let (mut project, mut history) = imported();
        let before = project.clone();
        assert!(history.apply(trim_end(&project, 0), &mut project).is_err());
        assert_eq!(project, before);
        assert!(history.undo(&mut project));
        assert_eq!(end_of(&project), Frame(0));
    }
}
