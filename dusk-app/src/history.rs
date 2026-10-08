//! The undo stack (docs/ARCHITECTURE.md, "Commands"): the commands applied so far and a
//! cursor; undo reverts the command before the cursor, redo applies the one after it again.

use dusk_core::{Command, Project, Rejection};

/// Applied commands, in order, and how many of them are in effect.
#[derive(Default)]
pub struct History {
    commands: Vec<Command>,
    cursor: usize,
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
        self.commands.push(command);
        self.cursor = self.commands.len();
        Ok(&self.commands[self.cursor - 1])
    }

    /// Reverts the last command in effect; false when there is none.
    pub fn undo(&mut self, project: &mut Project) -> bool {
        let Some(last) = self.cursor.checked_sub(1) else {
            return false;
        };
        self.commands[last].revert(project);
        self.cursor = last;
        true
    }

    /// Applies the last undone command again; false when there is none.
    pub fn redo(&mut self, project: &mut Project) -> Result<bool, Rejection> {
        let Some(command) = self.commands.get_mut(self.cursor) else {
            return Ok(false);
        };
        command.apply(project)?;
        self.cursor += 1;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{Edge, Frame, MediaInfo, MediaKind, MediaTime, Rational, TrimClips, import};

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
    fn a_refused_command_changes_nothing() {
        let (mut project, mut history) = imported();
        let before = project.clone();
        assert!(history.apply(trim_end(&project, 0), &mut project).is_err());
        assert_eq!(project, before);
        assert!(history.undo(&mut project));
        assert_eq!(end_of(&project), Frame(0));
    }
}
