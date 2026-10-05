//! Placing clips on tracks.

use crate::command::{Rejection, check_clip, overlaps, same_timing};
use crate::model::{Clip, Project, TrackId};

/// Places clips on tracks; linked clips go in together.
#[derive(Clone, Debug)]
pub struct InsertClips {
    placements: Vec<(TrackId, Clip)>,
}

impl InsertClips {
    /// Puts each clip on its track.
    pub fn new(placements: Vec<(TrackId, Clip)>) -> InsertClips {
        InsertClips { placements }
    }

    pub(super) fn apply(&self, project: &mut Project) -> Result<(), Rejection> {
        let tracks = self.check(project)?;
        for (index, (_, clip)) in tracks.into_iter().zip(&self.placements) {
            let clips = &mut project.sequence.tracks[index].clips;
            let at = clips.partition_point(|other| other.position < clip.position);
            clips.insert(at, clip.clone());
        }
        Ok(())
    }

    pub(super) fn revert(&self, project: &mut Project) {
        for (track_id, clip) in &self.placements {
            if let Some(track) = project.sequence.track_mut(*track_id) {
                track.clips.retain(|other| other.id != clip.id);
            }
        }
    }

    /// Checks every placement against the timeline invariants and returns the index of each
    /// placement's track.
    fn check(&self, project: &Project) -> Result<Vec<usize>, Rejection> {
        let tracks = &project.sequence.tracks;
        let mut indices = Vec::with_capacity(self.placements.len());
        for (number, (track_id, clip)) in self.placements.iter().enumerate() {
            let index = tracks
                .iter()
                .position(|track| track.id == *track_id)
                .ok_or(Rejection::UnknownTrack(*track_id))?;
            let track = &tracks[index];
            if track.locked {
                return Err(Rejection::TrackLocked(track.id));
            }
            if clip.kind() != track.kind {
                return Err(Rejection::WrongTrackKind(clip.id));
            }
            let earlier = &self.placements[..number];
            if project.find_clip(clip.id).is_some() || earlier.iter().any(|(_, c)| c.id == clip.id)
            {
                return Err(Rejection::DuplicateId);
            }
            check_clip(project, clip)?;
            let on_track = track.clips.iter();
            let placed = earlier
                .iter()
                .filter(|(id, _)| id == track_id)
                .map(|(_, c)| c);
            if on_track.chain(placed).any(|other| overlaps(other, clip)) {
                return Err(Rejection::Overlap(track.id));
            }
            indices.push(index);
        }
        for (_, clip) in &self.placements {
            let Some(link) = clip.link else { continue };
            let new = self.placements.iter().map(|(_, c)| c);
            let existing = project.clips().map(|(_, c)| c);
            let mut group = new.chain(existing).filter(|c| c.link == Some(link));
            if let Some(first) = group.next()
                && let Some(odd) = group.find(|other| !same_timing(first, other))
            {
                return Err(Rejection::LinkMismatch(odd.id));
            }
        }
        Ok(indices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use crate::command::testing::*;
    use crate::model::{AudioEdits, ClipEdits, ClipId, LinkId, MediaId, TrackKind};
    use crate::time::{Frame, MediaTime};

    #[test]
    fn inserting_a_linked_pair_puts_each_clip_on_its_track_and_reverts() {
        let mut project = project();
        let before = project.clone();
        let mut ids = project.fresh_ids();
        let link = ids.link();
        let mut video = Clip::new(
            ids.clip(),
            MediaId(1),
            TrackKind::Video,
            (MediaTime(0), MediaTime(2 * SECOND)),
            Frame(0),
            fps30(),
        );
        video.link = Some(link);
        let mut audio = video.clone();
        audio.id = ids.clip();
        audio.edits = ClipEdits::Audio(AudioEdits::default());
        let mut insert = Command::InsertClips(InsertClips::new(vec![
            (track(&project, TrackKind::Video), video.clone()),
            (track(&project, TrackKind::Audio), audio.clone()),
        ]));
        insert.apply(&mut project).unwrap();

        let (video_track, found) = project.find_clip(video.id).unwrap();
        assert_eq!((video_track.kind(), found), (TrackKind::Video, &video));
        let (audio_track, found) = project.find_clip(audio.id).unwrap();
        assert_eq!((audio_track.kind(), found), (TrackKind::Audio, &audio));
        assert_eq!(project.link_group(video.id).len(), 2);

        insert.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn clips_stay_sorted_by_position() {
        let mut project = project();
        insert_pair(&mut project, 300, (5 * SECOND, 6 * SECOND));
        insert_pair(&mut project, 0, (0, SECOND));
        let video = &project.sequence().tracks()[0];
        let positions: Vec<_> = video.clips().iter().map(|clip| clip.position).collect();
        assert_eq!(positions, [Frame(0), Frame(300)]);
    }

    #[test]
    fn inserting_refuses_bad_clips_and_changes_nothing() {
        let mut project = project();
        insert_pair(&mut project, 100, (0, 2 * SECOND)); // frames 100..160
        let before = project.clone();
        let video_track = track(&project, TrackKind::Video);
        let audio_track = track(&project, TrackKind::Audio);
        let base = Clip::new(
            project.fresh_ids().clip(),
            MediaId(1),
            TrackKind::Video,
            (MediaTime(0), MediaTime(SECOND)),
            Frame(0),
            fps30(),
        );
        let refused = |placements: Vec<(TrackId, Clip)>| {
            let mut project = before.clone();
            let result = Command::InsertClips(InsertClips::new(placements)).apply(&mut project);
            assert_eq!(project, before, "a refused insert changed the project");
            result.unwrap_err()
        };

        let mut wrong_media = base.clone();
        wrong_media.media_id = MediaId(9);
        assert_eq!(
            refused(vec![(video_track, wrong_media)]),
            Rejection::UnknownMedia(MediaId(9))
        );
        assert_eq!(
            refused(vec![(audio_track, base.clone())]),
            Rejection::WrongTrackKind(base.id)
        );
        assert_eq!(
            refused(vec![(TrackId(77), base.clone())]),
            Rejection::UnknownTrack(TrackId(77))
        );
        let mut overlapping = base.clone();
        overlapping.position = Frame(130);
        assert_eq!(
            refused(vec![(video_track, overlapping)]),
            Rejection::Overlap(video_track)
        );
        let mut late = base.clone();
        late.source_in = MediaTime(9 * SECOND);
        late.source_out = MediaTime(11 * SECOND);
        late.length = Frame(60);
        assert_eq!(
            refused(vec![(video_track, late)]),
            Rejection::SourceRange(base.id)
        );
        let mut stretched = base.clone();
        stretched.length = Frame(31);
        assert_eq!(
            refused(vec![(video_track, stretched)]),
            Rejection::Length(base.id)
        );
        let mut rushed = base.clone();
        rushed.speed = 40.0;
        assert_eq!(
            refused(vec![(video_track, rushed)]),
            Rejection::Speed(base.id)
        );
        let mut early = base.clone();
        early.position = Frame(-5);
        assert_eq!(
            refused(vec![(video_track, early)]),
            Rejection::BeforeStart(base.id)
        );
        let used = before.sequence().tracks()[0].clips()[0].id;
        let mut duplicate = base.clone();
        duplicate.id = used;
        assert_eq!(
            refused(vec![(video_track, duplicate)]),
            Rejection::DuplicateId
        );
        let mut first = base.clone();
        first.link = Some(LinkId(50));
        let mut second = first.clone();
        second.id = ClipId(base.id.0 + 1);
        second.edits = ClipEdits::Audio(AudioEdits::default());
        second.position = Frame(1);
        assert_eq!(
            refused(vec![(video_track, first), (audio_track, second)]),
            Rejection::LinkMismatch(ClipId(base.id.0 + 1))
        );
        let mut on_top = base.clone();
        on_top.id = ClipId(base.id.0 + 1);
        assert_eq!(
            refused(vec![(video_track, base.clone()), (video_track, on_top)]),
            Rejection::Overlap(video_track)
        );
    }

    #[test]
    fn a_still_clip_has_any_length_and_no_source_range() {
        let mut project = project();
        let photo = add_still(&mut project);
        let video_track = track(&project, TrackKind::Video);
        let still = Clip::still(project.fresh_ids().clip(), photo, Frame(0), Frame(7));
        let refused = |clip: Clip| {
            let mut project = project.clone();
            Command::InsertClips(InsertClips::new(vec![(video_track, clip)])).apply(&mut project)
        };
        assert_eq!(refused(still.clone()), Ok(()));
        let mut ranged = still.clone();
        ranged.source_out = MediaTime(SECOND);
        assert_eq!(refused(ranged), Err(Rejection::SourceRange(still.id)));
        let mut empty = still.clone();
        empty.length = Frame(0);
        assert_eq!(refused(empty), Err(Rejection::TooShort(still.id)));
        let mut fast = still.clone();
        fast.speed = 2.0;
        assert_eq!(refused(fast), Err(Rejection::Speed(still.id)));
    }

    #[test]
    fn inserting_on_a_locked_track_is_refused() {
        let mut project = project();
        project.sequence.tracks[0].locked = true;
        let video_track = track(&project, TrackKind::Video);
        let clip = Clip::new(
            project.fresh_ids().clip(),
            MediaId(1),
            TrackKind::Video,
            (MediaTime(0), MediaTime(SECOND)),
            Frame(0),
            fps30(),
        );
        assert_eq!(
            Command::InsertClips(InsertClips::new(vec![(video_track, clip)])).apply(&mut project),
            Err(Rejection::TrackLocked(video_track))
        );
    }
}
