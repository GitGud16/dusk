//! Moving a link group along the timeline, and to another track.

use crate::command::{Rejection, group_indices, overlaps};
use crate::model::{Clip, ClipId, Project, TrackId};
use crate::time::Frame;

/// Moves a clip and every clip linked to it to a new position, keeping them in lock-step;
/// the clip itself may change to another track of its kind, its partners keep theirs.
#[derive(Clone, Debug)]
pub struct MoveClips {
    clip: ClipId,
    to: Frame,
    track: Option<TrackId>,
    before: Vec<(TrackId, Clip)>,
}

impl MoveClips {
    /// Moves `clip` and its partners to start at `to`, and `clip` onto `track` if given.
    pub fn new(clip: ClipId, to: Frame, track: Option<TrackId>) -> MoveClips {
        MoveClips {
            clip,
            to,
            track,
            before: Vec::new(),
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        let plan = placements(project, self.clip, self.track)?;
        let tracks = &project.sequence.tracks;
        for &(_, from, to) in &plan {
            for index in [from, to] {
                if tracks[index].locked {
                    return Err(Rejection::TrackLocked(tracks[index].id));
                }
            }
        }
        if self.to < Frame(0) {
            return Err(Rejection::BeforeStart(self.clip));
        }
        let members: Vec<ClipId> = plan.iter().map(|(clip, _, _)| clip.id).collect();
        for (clip, _, to) in &plan {
            let moved = Clip {
                position: self.to,
                ..clip.clone()
            };
            let others = tracks[*to]
                .clips
                .iter()
                .filter(|other| !members.contains(&other.id));
            if others.into_iter().any(|other| overlaps(other, &moved)) {
                return Err(Rejection::Overlap(tracks[*to].id));
            }
        }
        self.before = plan
            .iter()
            .map(|(clip, from, _)| (tracks[*from].id, clip.clone()))
            .collect();
        let tracks = &mut project.sequence.tracks;
        for (clip, from, _) in &plan {
            tracks[*from].clips.retain(|other| other.id != clip.id);
        }
        for (clip, _, to) in plan {
            let moved = Clip {
                position: self.to,
                ..clip
            };
            insert_sorted(&mut tracks[to].clips, moved);
        }
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        let ids: Vec<ClipId> = self.before.iter().map(|(_, clip)| clip.id).collect();
        for track in &mut project.sequence.tracks {
            track.clips.retain(|clip| !ids.contains(&clip.id));
        }
        for (track, clip) in self.before.drain(..) {
            if let Some(track) = project.sequence.track_mut(track) {
                insert_sorted(&mut track.clips, clip);
            }
        }
    }
}

/// Where each clip of `clip`'s group goes: the clip, the index of its track and the index
/// of the track it moves to (`track` for `clip` itself, if given).
fn placements(
    project: &Project,
    clip: ClipId,
    track: Option<TrackId>,
) -> Result<Vec<(Clip, usize, usize)>, Rejection> {
    let tracks = &project.sequence.tracks;
    let destination = match track {
        Some(id) => Some(
            tracks
                .iter()
                .position(|track| track.id == id)
                .ok_or(Rejection::UnknownTrack(id))?,
        ),
        None => None,
    };
    group_indices(project, clip)?
        .into_iter()
        .map(|(t, i)| {
            let member = tracks[t].clips[i].clone();
            let to = match destination {
                Some(to) if member.id == clip => {
                    if tracks[to].kind != member.kind() {
                        return Err(Rejection::WrongTrackKind(member.id));
                    }
                    to
                }
                _ => t,
            };
            Ok((member, t, to))
        })
        .collect()
}

/// Inserts `clip` into `clips`, which are sorted by position, keeping them sorted.
fn insert_sorted(clips: &mut Vec<Clip>, clip: Clip) {
    let at = clips.partition_point(|other| other.position < clip.position);
    clips.insert(at, clip);
}

/// The position nearest to `wanted` where `clip` and its partners fit without overlapping
/// another clip, with `clip` on `track` if given: where the timeline snaps a drag before it
/// moves the clips. `None` for an unknown clip or track.
pub fn nearest_free_position(
    project: &Project,
    clip: ClipId,
    wanted: Frame,
    track: Option<TrackId>,
) -> Option<Frame> {
    let plan = placements(project, clip, track).ok()?;
    let members: Vec<ClipId> = plan.iter().map(|(clip, _, _)| clip.id).collect();
    let length = plan.first()?.0.length.0;
    // Start positions where the group fits, as inclusive ranges; `None` is unbounded.
    let mut valid: Vec<(i64, Option<i64>)> = vec![(0, None)];
    let mut destinations: Vec<usize> = plan.iter().map(|&(_, _, to)| to).collect();
    destinations.dedup();
    for to in destinations {
        let others = project.sequence.tracks[to]
            .clips
            .iter()
            .filter(|other| !members.contains(&other.id));
        let mut free = Vec::new();
        let mut cursor = 0;
        for other in others {
            if other.position.0 - length >= cursor {
                free.push((cursor, Some(other.position.0 - length)));
            }
            cursor = cursor.max(other.end().0);
        }
        free.push((cursor, None));
        valid = intersect(&valid, &free);
    }
    let wanted = wanted.0.max(0);
    valid
        .iter()
        .map(|&(low, high)| {
            let at = wanted.clamp(low, high.unwrap_or(i64::MAX));
            (at.abs_diff(wanted), at)
        })
        .min()
        .map(|(_, at)| Frame(at))
}

/// The ranges both `a` and `b` contain.
fn intersect(a: &[(i64, Option<i64>)], b: &[(i64, Option<i64>)]) -> Vec<(i64, Option<i64>)> {
    let mut both = Vec::new();
    for &(a_low, a_high) in a {
        for &(b_low, b_high) in b {
            let low = a_low.max(b_low);
            let high = match (a_high, b_high) {
                (Some(x), Some(y)) => Some(x.min(y)),
                (x, y) => x.or(y),
            };
            if high.is_none_or(|high| low <= high) {
                both.push((low, high));
            }
        }
    }
    both
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::command::testing::*;
    use crate::model::TrackKind;

    fn video_tracks(project: &Project) -> (TrackId, TrackId) {
        let tracks = project.sequence().tracks();
        (tracks[0].id(), tracks[1].id())
    }

    fn moved(project: &mut Project, id: ClipId, to: i64, track: Option<TrackId>) -> Command {
        let mut command = Command::MoveClips(MoveClips::new(id, Frame(to), track));
        command.apply(project).unwrap();
        command
    }

    #[test]
    fn a_linked_pair_moves_together_and_reverts() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, SECOND)); // 0..30
        let before = project.clone();
        let mut command = moved(&mut project, audio, 90, None);
        for id in [video, audio] {
            assert_eq!(clip(&project, id).position, Frame(90));
        }
        assert_eq!(
            project.find_clip(video).unwrap().0.id(),
            track(&before, TrackKind::Video)
        );
        command.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn a_clip_moves_to_another_track_and_its_partner_stays_on_its_own() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, SECOND));
        let (v1, v2) = video_tracks(&project);
        let before = project.clone();
        let mut command = moved(&mut project, video, 15, Some(v2));
        assert_eq!(project.find_clip(video).unwrap().0.id(), v2);
        assert!(project.sequence().track(v1).unwrap().clips().is_empty());
        assert_eq!(
            project.find_clip(audio).unwrap().0.id(),
            track(&before, TrackKind::Audio)
        );
        assert_eq!(clip(&project, audio).position, Frame(15));
        command.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn moves_that_break_an_invariant_are_refused() {
        let mut project = project();
        let (first, _) = insert_pair(&mut project, 0, (0, SECOND)); // 0..30
        insert_pair(&mut project, 60, (2 * SECOND, 3 * SECOND)); // 60..90
        let audio_track = track(&project, TrackKind::Audio);
        let video_track = track(&project, TrackKind::Video);
        let before = project.clone();
        let refused = |to, track| {
            let mut project = before.clone();
            let result =
                Command::MoveClips(MoveClips::new(first, Frame(to), track)).apply(&mut project);
            assert_eq!(project, before, "a refused move changed the project");
            result.unwrap_err()
        };
        assert_eq!(refused(45, None), Rejection::Overlap(video_track));
        assert_eq!(refused(-1, None), Rejection::BeforeStart(first));
        assert_eq!(
            refused(200, Some(audio_track)),
            Rejection::WrongTrackKind(first)
        );
        assert_eq!(
            refused(200, Some(TrackId(99))),
            Rejection::UnknownTrack(TrackId(99))
        );
        let mut locked = before.clone();
        lock(&mut locked, audio_track);
        assert_eq!(
            Command::MoveClips(MoveClips::new(first, Frame(200), None)).apply(&mut locked),
            Err(Rejection::TrackLocked(audio_track))
        );
    }

    #[test]
    fn a_drag_snaps_to_the_nearest_place_where_the_whole_group_fits() {
        let mut project = project();
        let (moving, _) = insert_pair(&mut project, 0, (0, SECOND)); // 30 frames at 0
        insert_pair(&mut project, 100, (2 * SECOND, 4 * SECOND)); // 100..160
        let snap = |wanted| nearest_free_position(&project, moving, Frame(wanted), None);
        // Free already, or overlapping only its own old place.
        assert_eq!(snap(10), Some(Frame(10)));
        assert_eq!(snap(200), Some(Frame(200)));
        // Overlapping the other pair: nearer to its start, or to its end.
        assert_eq!(snap(90), Some(Frame(70)));
        assert_eq!(snap(150), Some(Frame(160)));
        assert_eq!(snap(-20), Some(Frame(0)));
        assert_eq!(
            nearest_free_position(&project, ClipId(99), Frame(0), None),
            None
        );
    }

    #[test]
    fn snapping_counts_every_track_the_group_lands_on() {
        let mut project = project();
        let (moving, _) = insert_pair(&mut project, 300, (0, SECOND)); // 30 frames
        let audio_track = track(&project, TrackKind::Audio);
        // An audio-only clip at 100..130 blocks the pair there, even with no video clip.
        let alone = crate::model::Clip::new(
            project.fresh_ids().clip(),
            crate::model::MediaId(1),
            TrackKind::Audio,
            (crate::time::MediaTime(0), crate::time::MediaTime(SECOND)),
            Frame(100),
            fps30(),
        );
        Command::InsertClips(crate::command::InsertClips::new(vec![(audio_track, alone)]))
            .apply(&mut project)
            .unwrap();
        assert_eq!(
            nearest_free_position(&project, moving, Frame(105), None),
            Some(Frame(130))
        );
        let (_, v2) = video_tracks(&project);
        // Moving the video onto V2 changes nothing for the audio, which stays on A1.
        assert_eq!(
            nearest_free_position(&project, moving, Frame(105), Some(v2)),
            Some(Frame(130))
        );
    }
}
