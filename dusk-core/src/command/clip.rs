//! Changing clips in place: enabling and disabling one clip, and unlinking a link group.

use crate::command::{Rejection, clip_indices, group_indices};
use crate::model::{ClipId, LinkId, Project};

/// Enables or disables one clip. A disabled clip is skipped in preview and export: its
/// picture is hidden, its sound silent. Its link partners stay as they are.
#[derive(Clone, Debug)]
pub struct SetClipEnabled {
    clip: ClipId,
    enabled: bool,
    before: bool,
}

impl SetClipEnabled {
    /// Enables `clip`, or disables it when `enabled` is false.
    pub fn new(clip: ClipId, enabled: bool) -> SetClipEnabled {
        SetClipEnabled {
            clip,
            enabled,
            before: true,
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        let (t, i) = clip_indices(project, self.clip)?;
        let track = &mut project.sequence.tracks[t];
        if track.locked {
            return Err(Rejection::TrackLocked(track.id));
        }
        self.before = track.clips[i].enabled;
        track.clips[i].enabled = self.enabled;
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        if let Ok((t, i)) = clip_indices(project, self.clip) {
            project.sequence.tracks[t].clips[i].enabled = self.before;
        }
    }
}

/// Separates every clip linked to a clip ("detach audio"), so each can be edited alone.
#[derive(Clone, Debug)]
pub struct Unlink {
    clip: ClipId,
    /// The clips that were linked, and their link.
    before: Vec<(ClipId, LinkId)>,
}

impl Unlink {
    /// Unlinks `clip` from its link group.
    pub fn new(clip: ClipId) -> Unlink {
        Unlink {
            clip,
            before: Vec::new(),
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        let (t, i) = clip_indices(project, self.clip)?;
        if project.sequence.tracks[t].clips[i].link.is_none() {
            return Err(Rejection::NotLinked(self.clip));
        }
        let members = group_indices(project, self.clip)?;
        for &(t, _) in &members {
            let track = &project.sequence.tracks[t];
            if track.locked {
                return Err(Rejection::TrackLocked(track.id));
            }
        }
        self.before.clear();
        for (t, i) in members {
            let clip = &mut project.sequence.tracks[t].clips[i];
            if let Some(link) = clip.link.take() {
                self.before.push((clip.id, link));
            }
        }
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        for (id, link) in self.before.drain(..) {
            if let Ok((t, i)) = clip_indices(project, id) {
                project.sequence.tracks[t].clips[i].link = Some(link);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::*;
    use crate::command::{Command, Edge};
    use crate::model::TrackKind;
    use crate::time::Frame;

    #[test]
    fn a_disabled_clip_is_hidden_and_its_partner_stays() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, SECOND));
        let before = project.clone();
        let mut disable = Command::SetClipEnabled(SetClipEnabled::new(video, false));
        disable.apply(&mut project).unwrap();
        assert!(!clip(&project, video).enabled);
        assert!(clip(&project, audio).enabled);
        assert!(project.sequence().visible_video_at(Frame(5)).is_none());
        disable.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn a_clip_on_a_locked_track_cannot_be_disabled() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 0, (0, SECOND));
        let video_track = track(&project, TrackKind::Video);
        lock(&mut project, video_track);
        assert_eq!(
            Command::SetClipEnabled(SetClipEnabled::new(video, false)).apply(&mut project),
            Err(Rejection::TrackLocked(video_track))
        );
        assert_eq!(
            Command::SetClipEnabled(SetClipEnabled::new(ClipId(99), false)).apply(&mut project),
            Err(Rejection::UnknownClip(ClipId(99)))
        );
    }

    #[test]
    fn unlinked_clips_are_edited_alone_and_relink_on_revert() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 4 * SECOND));
        let before = project.clone();
        let mut unlink = Command::Unlink(Unlink::new(audio));
        unlink.apply(&mut project).unwrap();
        assert_eq!(project.link_group(video), [video]);
        assert_eq!(clip(&project, audio).link, None);
        trim(&mut project, video, Edge::End, 30).unwrap();
        assert_eq!(clip(&project, audio).length, Frame(120));
        assert_eq!(clip(&project, video).length, Frame(30));

        let mut project = before.clone();
        unlink.apply(&mut project).unwrap();
        unlink.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn unlinking_needs_a_link_and_unlocked_tracks() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 0, (0, SECOND));
        let audio_track = track(&project, TrackKind::Audio);
        let mut locked = project.clone();
        lock(&mut locked, audio_track);
        assert_eq!(
            Command::Unlink(Unlink::new(video)).apply(&mut locked),
            Err(Rejection::TrackLocked(audio_track))
        );
        Command::Unlink(Unlink::new(video))
            .apply(&mut project)
            .unwrap();
        assert_eq!(
            Command::Unlink(Unlink::new(video)).apply(&mut project),
            Err(Rejection::NotLinked(video))
        );
    }
}
