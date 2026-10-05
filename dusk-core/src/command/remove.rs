//! Deleting a link group, with or without closing the gap (ripple).

use crate::command::{Rejection, group_indices};
use crate::model::{Clip, ClipId, Project, TrackId};

/// Deletes a clip and every clip linked to it. A plain delete leaves a gap; a ripple delete
/// closes it by moving every later clip on every unlocked track left by the deleted length
/// (docs/ARCHITECTURE.md, "Ripple delete"). Alt+Delete is an [`Unlink`](crate::Unlink)
/// followed by this, in one batch.
#[derive(Clone, Debug)]
pub struct RemoveClips {
    clip: ClipId,
    ripple: bool,
    /// The clips of every track the command changed, as they were.
    before: Vec<(TrackId, Vec<Clip>)>,
}

impl RemoveClips {
    /// Deletes `clip` and its partners, closing the gap when `ripple` is set.
    pub fn new(clip: ClipId, ripple: bool) -> RemoveClips {
        RemoveClips {
            clip,
            ripple,
            before: Vec::new(),
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        let members = group_indices(project, self.clip)?;
        let tracks = &project.sequence.tracks;
        for &(t, _) in &members {
            if tracks[t].locked {
                return Err(Rejection::TrackLocked(tracks[t].id));
            }
        }
        let ids: Vec<ClipId> = members
            .iter()
            .map(|&(t, i)| tracks[t].clips[i].id)
            .collect();
        let (t, i) = members[0];
        let (start, end) = (tracks[t].clips[i].position, tracks[t].clips[i].end());
        let mut touched: Vec<usize> = members.iter().map(|&(t, _)| t).collect();
        if self.ripple {
            for (t, track) in tracks.iter().enumerate().filter(|(_, track)| !track.locked) {
                let others = track.clips.iter().filter(|clip| !ids.contains(&clip.id));
                if others
                    .into_iter()
                    .any(|clip| clip.position < end && start < clip.end())
                {
                    return Err(Rejection::RippleBlocked(track.id));
                }
                touched.push(t);
            }
            // A clip that moves takes its link partners along, so none may be locked in place.
            let moving = tracks
                .iter()
                .filter(|track| !track.locked)
                .flat_map(|track| &track.clips)
                .filter(|clip| !ids.contains(&clip.id) && clip.position >= end);
            for link in moving.filter_map(|clip| clip.link) {
                let held = tracks.iter().find(|track| {
                    track.locked && track.clips.iter().any(|clip| clip.link == Some(link))
                });
                if let Some(track) = held {
                    return Err(Rejection::TrackLocked(track.id));
                }
            }
        }
        touched.sort_unstable();
        touched.dedup();
        self.before = touched
            .iter()
            .map(|&t| (tracks[t].id, tracks[t].clips.clone()))
            .collect();
        let length = end - start;
        for t in touched {
            let track = &mut project.sequence.tracks[t];
            track.clips.retain(|clip| !ids.contains(&clip.id));
            if self.ripple && !track.locked {
                for clip in track.clips.iter_mut().filter(|clip| clip.position >= end) {
                    clip.position = clip.position - length;
                }
            }
        }
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        for (track, clips) in self.before.drain(..) {
            if let Some(track) = project.sequence.track_mut(track) {
                track.clips = clips;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::*;
    use crate::command::{Command, InsertClips, Unlink};
    use crate::model::{MediaId, TrackKind};
    use crate::time::{Frame, MediaTime};

    fn remove(project: &mut Project, id: ClipId, ripple: bool) -> Result<Command, Rejection> {
        let mut command = Command::RemoveClips(RemoveClips::new(id, ripple));
        command.apply(project).map(|()| command)
    }

    fn positions(project: &Project, track: TrackId) -> Vec<i64> {
        let clips = project.sequence().track(track).unwrap().clips();
        clips.iter().map(|clip| clip.position.0).collect()
    }

    /// An audio-only clip of one second at `position` on A2.
    fn on_a2(project: &mut Project, position: i64) -> ClipId {
        let a2 = project.sequence().tracks()[3].id();
        let clip = Clip::new(
            project.fresh_ids().clip(),
            MediaId(1),
            TrackKind::Audio,
            (MediaTime(0), MediaTime(SECOND)),
            Frame(position),
            fps30(),
        );
        let id = clip.id;
        Command::InsertClips(InsertClips::new(vec![(a2, clip)]))
            .apply(project)
            .unwrap();
        id
    }

    #[test]
    fn a_plain_delete_removes_the_group_and_leaves_a_gap() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, SECOND)); // 0..30
        insert_pair(&mut project, 60, (2 * SECOND, 3 * SECOND)); // 60..90
        let before = project.clone();
        let mut command = remove(&mut project, audio, false).unwrap();
        assert!(project.find_clip(video).is_none() && project.find_clip(audio).is_none());
        assert_eq!(positions(&project, track(&project, TrackKind::Video)), [60]);
        command.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn a_ripple_delete_closes_the_gap_on_every_unlocked_track() {
        let mut project = project();
        let (first, _) = insert_pair(&mut project, 0, (0, SECOND)); // 0..30
        insert_pair(&mut project, 60, (2 * SECOND, 3 * SECOND)); // 60..90
        let a2_clip = on_a2(&mut project, 40); // 40..70, after the deleted range
        let before = project.clone();
        let mut command = remove(&mut project, first, true).unwrap();
        let video_track = track(&project, TrackKind::Video);
        assert_eq!(positions(&project, video_track), [30]);
        assert_eq!(clip(&project, a2_clip).position, Frame(10));
        command.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn a_locked_track_keeps_its_clips_in_place() {
        let mut project = project();
        let (first, _) = insert_pair(&mut project, 0, (0, SECOND)); // 0..30
        let a2_clip = on_a2(&mut project, 40);
        let a2 = project.sequence().tracks()[3].id();
        lock(&mut project, a2);
        remove(&mut project, first, true).unwrap();
        assert_eq!(clip(&project, a2_clip).position, Frame(40));
    }

    #[test]
    fn a_ripple_that_would_create_an_overlap_is_refused() {
        let mut project = project();
        let (first, _) = insert_pair(&mut project, 0, (0, 2 * SECOND)); // 0..60
        on_a2(&mut project, 30); // 30..60: overlaps the deleted range
        let before = project.clone();
        let a2 = project.sequence().tracks()[3].id();
        assert_eq!(
            remove(&mut project, first, true).map(|_| ()),
            Err(Rejection::RippleBlocked(a2))
        );
        assert_eq!(project, before);
        // A plain delete is fine.
        remove(&mut project, first, false).unwrap();
    }

    #[test]
    fn a_ripple_that_would_split_a_link_group_is_refused() {
        let mut project = project();
        let (first, _) = insert_pair(&mut project, 0, (0, SECOND)); // V1 and A1, 0..30
        let (later, _) = insert_pair(&mut project, 60, (2 * SECOND, 3 * SECOND));
        let audio_track = track(&project, TrackKind::Audio);
        // Unlink the first pair so deleting its video leaves A1 untouched otherwise.
        Command::Unlink(Unlink::new(first))
            .apply(&mut project)
            .unwrap();
        lock(&mut project, audio_track);
        // The later pair's audio is on locked A1, so its video cannot move without it.
        assert_eq!(
            remove(&mut project, first, true).map(|_| ()),
            Err(Rejection::TrackLocked(audio_track))
        );
        assert_eq!(clip(&project, later).position, Frame(60));
    }

    #[test]
    fn deleting_from_a_locked_track_is_refused() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 0, (0, SECOND));
        let video_track = track(&project, TrackKind::Video);
        lock(&mut project, video_track);
        assert_eq!(
            remove(&mut project, video, false).map(|_| ()),
            Err(Rejection::TrackLocked(video_track))
        );
    }

    #[test]
    fn alt_delete_removes_only_the_one_clip() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, SECOND));
        let before = project.clone();
        let mut command = Command::Batch(vec![
            Command::Unlink(Unlink::new(video)),
            Command::RemoveClips(RemoveClips::new(video, false)),
        ]);
        command.apply(&mut project).unwrap();
        assert!(project.find_clip(video).is_none());
        assert_eq!(clip(&project, audio).link, None);
        command.revert(&mut project);
        assert_eq!(project, before);
    }
}
