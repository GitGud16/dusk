//! Locking and muting tracks.

use crate::command::Rejection;
use crate::model::{Project, Track, TrackId};

/// Locks or unlocks a track. No command may change the clips of a locked track, and ripple
/// shifts pass it by.
#[derive(Clone, Debug)]
pub struct SetTrackLocked {
    track: TrackId,
    locked: bool,
    before: bool,
}

impl SetTrackLocked {
    /// Locks `track`, or unlocks it when `locked` is false.
    pub fn new(track: TrackId, locked: bool) -> SetTrackLocked {
        SetTrackLocked {
            track,
            locked,
            before: false,
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        let track = track_mut(project, self.track)?;
        self.before = track.locked;
        track.locked = self.locked;
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        if let Ok(track) = track_mut(project, self.track) {
            track.locked = self.before;
        }
    }
}

/// Mutes or unmutes a track: a muted track is left out of preview and export (a muted video
/// track hides all its clips).
#[derive(Clone, Debug)]
pub struct SetTrackMuted {
    track: TrackId,
    muted: bool,
    before: bool,
}

impl SetTrackMuted {
    /// Mutes `track`, or unmutes it when `muted` is false.
    pub fn new(track: TrackId, muted: bool) -> SetTrackMuted {
        SetTrackMuted {
            track,
            muted,
            before: false,
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        let track = track_mut(project, self.track)?;
        self.before = track.muted;
        track.muted = self.muted;
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        if let Ok(track) = track_mut(project, self.track) {
            track.muted = self.before;
        }
    }
}

/// The track with `id`, or the rejection naming it.
fn track_mut(project: &mut Project, id: TrackId) -> Result<&mut Track, Rejection> {
    project
        .sequence
        .track_mut(id)
        .ok_or(Rejection::UnknownTrack(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::*;
    use crate::command::{Command, InsertClips};
    use crate::model::{Clip, MediaId, TrackKind};
    use crate::time::{Frame, MediaTime};

    fn video_clip(project: &Project) -> Clip {
        Clip::new(
            project.fresh_ids().clip(),
            MediaId(1),
            TrackKind::Video,
            (MediaTime(0), MediaTime(SECOND)),
            Frame(0),
            fps30(),
        )
    }

    #[test]
    fn a_locked_track_refuses_edits_until_unlocked() {
        let mut project = project();
        let before = project.clone();
        let video = track(&project, TrackKind::Video);
        let mut lock = Command::SetTrackLocked(SetTrackLocked::new(video, true));
        lock.apply(&mut project).unwrap();
        assert!(project.sequence().track(video).unwrap().locked());
        let insert = |project: &mut Project| {
            let clip = video_clip(project);
            Command::InsertClips(InsertClips::new(vec![(video, clip)])).apply(project)
        };
        assert_eq!(
            insert(&mut project.clone()),
            Err(Rejection::TrackLocked(video))
        );
        lock.revert(&mut project);
        assert_eq!(project, before);
        assert_eq!(insert(&mut project), Ok(()));
    }

    #[test]
    fn a_muted_video_track_hides_its_clips() {
        let mut project = project();
        insert_pair(&mut project, 0, (0, SECOND));
        let video = track(&project, TrackKind::Video);
        let before = project.clone();
        let mut mute = Command::SetTrackMuted(SetTrackMuted::new(video, true));
        mute.apply(&mut project).unwrap();
        assert!(project.sequence().track(video).unwrap().muted());
        assert!(project.sequence().visible_video_at(Frame(10)).is_none());
        mute.revert(&mut project);
        assert_eq!(project, before);
        assert!(project.sequence().visible_video_at(Frame(10)).is_some());
    }

    #[test]
    fn an_unknown_track_is_refused() {
        let mut project = project();
        assert_eq!(
            Command::SetTrackMuted(SetTrackMuted::new(TrackId(42), true)).apply(&mut project),
            Err(Rejection::UnknownTrack(TrackId(42)))
        );
        assert_eq!(
            Command::SetTrackLocked(SetTrackLocked::new(TrackId(42), true)).apply(&mut project),
            Err(Rejection::UnknownTrack(TrackId(42)))
        );
    }
}
