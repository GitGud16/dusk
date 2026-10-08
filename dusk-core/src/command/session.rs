//! Applying the clip editor's draft to its link group as one edit (docs/ARCHITECTURE.md,
//! "Pop-out clip editor").

use crate::command::{Notice, Rejection, check_clip, group_indices, next_start};
use crate::model::{Clip, ClipEdits, ClipId, MediaKind, Project, TrackId};
use crate::session::ClipDraft;
use crate::time::source_span;

/// Sets the trim, the speed, the picture and sound edits and, for a still, the length of
/// every clip of a link group to a draft, keeping each clip where it starts. A draft longer
/// than the room before the next clip is cut at the gap. Undoing restores the clips as they
/// were when it was applied, whatever happened to them while the draft was made.
#[derive(Clone, Debug)]
pub struct ApplyClipSession {
    group: Vec<ClipId>,
    after: ClipDraft,
    before: Vec<(TrackId, Clip)>,
    pub(super) notices: Vec<Notice>,
}

impl ApplyClipSession {
    /// Applies `after` to the clips `group`, which must still be one link group.
    pub fn new(group: Vec<ClipId>, after: ClipDraft) -> ApplyClipSession {
        ApplyClipSession {
            group,
            after,
            before: Vec::new(),
            notices: Vec::new(),
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        let first = *self.group.first().ok_or(Rejection::GroupChanged)?;
        // The draft was made for this link group; one that lost or gained a clip, or was
        // unlinked, is another group now.
        let group = project.link_group(first);
        if group.len() != self.group.len() || !self.group.iter().all(|id| group.contains(id)) {
            return Err(Rejection::GroupChanged);
        }
        let members = group_indices(project, first)?;
        for &(t, _) in &members {
            let track = &project.sequence.tracks[t];
            if track.locked {
                return Err(Rejection::TrackLocked(track.id));
            }
        }
        let (t, i) = members[0];
        let lead = &project.sequence.tracks[t].clips[i];
        let still = project
            .media_ref(lead.media_id)
            .ok_or(Rejection::UnknownMedia(lead.media_id))?
            .info
            .kind
            == MediaKind::Still;
        let rate = project.sequence.frame_rate;
        let draft = &self.after;
        let wanted = draft.length(still, rate);
        // Each clip with the draft as asked, checked like any clip on the timeline.
        for &(t, i) in &members {
            check_clip(
                project,
                &draft.applied_to(&project.sequence.tracks[t].clips[i], still, wanted),
            )?;
        }
        // Kept where it starts, it ends at the next clip at the latest (cut at the gap).
        let mut notices = Vec::new();
        let mut length = wanted;
        let next = members
            .iter()
            .filter_map(|&(t, i)| next_start(project, t, i))
            .min();
        if let Some(gap_end) = next
            && lead.position + length > gap_end
        {
            length = gap_end - lead.position;
            notices.push(Notice::CutAtGap(first));
        }
        let source_out = if still || length == wanted {
            draft.source_out
        } else {
            (draft.source_in + source_span(length, draft.speed, rate)).min(draft.source_out)
        };
        let after: Vec<Clip> = members
            .iter()
            .map(|&(t, i)| {
                let mut clip =
                    draft.applied_to(&project.sequence.tracks[t].clips[i], still, length);
                if !still {
                    clip.source_out = source_out;
                }
                // Fades longer than the cut clip are shortened to fit, fade-in first.
                if let ClipEdits::Audio(edits) = &mut clip.edits {
                    edits.fit_into(length);
                }
                clip
            })
            .collect();
        self.before = members
            .iter()
            .map(|&(t, i)| {
                let track = &project.sequence.tracks[t];
                (track.id, track.clips[i].clone())
            })
            .collect();
        self.notices = notices;
        for (&(t, i), clip) in members.iter().zip(after) {
            project.sequence.tracks[t].clips[i] = clip;
        }
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        for (track_id, before) in self.before.drain(..) {
            let clip = project
                .sequence
                .track_mut(track_id)
                .and_then(|track| track.clips.iter_mut().find(|clip| clip.id == before.id));
            if let Some(clip) = clip {
                *clip = before;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::*;
    use crate::command::{Command, Edge, remove_one};
    use crate::model::{ClipEdits, Rect, Rotation, TrackKind};
    use crate::session::ClipEditSession;
    use crate::time::{Frame, MediaTime};

    fn draft_of(project: &Project, clip: ClipId) -> ClipDraft {
        ClipEditSession::open(project, clip).unwrap().draft
    }

    fn apply(
        project: &mut Project,
        group: Vec<ClipId>,
        draft: ClipDraft,
    ) -> Result<Command, Rejection> {
        let mut command = Command::ApplyClipSession(ApplyClipSession::new(group, draft));
        command.apply(project).map(|()| command)
    }

    #[test]
    fn a_draft_changes_the_whole_group_in_one_step_and_reverts() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 30, (0, 10 * SECOND));
        let before = project.clone();
        let mut draft = draft_of(&project, video);
        draft.source_in = MediaTime(SECOND);
        draft.source_out = MediaTime(4 * SECOND);
        draft.speed = 2.0;
        draft.video.as_mut().unwrap().rotate = Rotation::Quarter;
        draft.audio.as_mut().unwrap().volume_db = -6.0;
        let mut command = apply(&mut project, vec![video, audio], draft).unwrap();
        for id in [video, audio] {
            let clip = clip(&project, id);
            // 3 s of source at twice the speed last 1.5 s, from where the clip was.
            assert_eq!((clip.position, clip.length), (Frame(30), Frame(45)));
            assert_eq!(
                (clip.source_in, clip.source_out, clip.speed),
                (MediaTime(SECOND), MediaTime(4 * SECOND), 2.0)
            );
        }
        assert!(matches!(
            &clip(&project, video).edits,
            ClipEdits::Video(edits) if edits.rotate == Rotation::Quarter
        ));
        assert!(matches!(
            &clip(&project, audio).edits,
            ClipEdits::Audio(edits) if edits.volume_db == -6.0
        ));
        assert!(command.notices().is_empty());
        command.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn a_draft_longer_than_the_room_is_cut_at_the_gap() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 5 * SECOND)); // 0..150
        insert_pair(&mut project, 200, (6 * SECOND, 7 * SECOND));
        let mut draft = draft_of(&project, video);
        draft.source_out = MediaTime(9 * SECOND); // 270 frames
        let command = apply(&mut project, vec![video, audio], draft).unwrap();
        let clip = clip(&project, video);
        assert_eq!(clip.end(), Frame(200));
        assert_eq!(clip.source_out, MediaTime(6_666_667));
        assert_eq!(command.notices(), [Notice::CutAtGap(video)]);
    }

    #[test]
    fn fades_that_no_longer_fit_after_the_cut_are_shortened() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 5 * SECOND));
        insert_pair(&mut project, 200, (6 * SECOND, 7 * SECOND));
        let mut draft = draft_of(&project, audio);
        draft.source_out = MediaTime(9 * SECOND);
        let edits = draft.audio.as_mut().unwrap();
        edits.fade_in = Frame(150);
        edits.fade_out = Frame(100);
        apply(&mut project, vec![video, audio], draft).unwrap();
        let ClipEdits::Audio(edits) = clip(&project, audio).edits else {
            panic!("an audio clip");
        };
        assert_eq!((edits.fade_in, edits.fade_out), (Frame(150), Frame(50)));
    }

    #[test]
    fn a_still_draft_sets_how_long_it_lasts() {
        let mut project = project();
        let photo = add_still(&mut project);
        let still = insert_still(&mut project, photo, 0, 150);
        let mut draft = draft_of(&project, still);
        draft.still_length = Frame(90);
        apply(&mut project, vec![still], draft).unwrap();
        assert_eq!(clip(&project, still).length, Frame(90));
    }

    #[test]
    fn drafts_that_break_a_rule_are_refused() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 5 * SECOND));
        let group = vec![video, audio];
        let base = draft_of(&project, video);
        let before = project.clone();
        let refused = |change: &dyn Fn(&mut ClipDraft)| {
            let mut project = before.clone();
            let mut draft = base.clone();
            change(&mut draft);
            let result = apply(&mut project, group.clone(), draft).map(|_| ());
            assert_eq!(project, before, "a refused draft changed the project");
            result.unwrap_err()
        };
        assert!(matches!(refused(&|d| d.speed = 40.0), Rejection::Speed(_)));
        assert!(matches!(
            refused(&|d| d.source_out = MediaTime(11 * SECOND)),
            Rejection::SourceRange(_)
        ));
        assert!(matches!(
            refused(&|d| d.source_in = d.source_out),
            Rejection::SourceRange(_)
        ));
        assert!(matches!(
            refused(&|d| d.audio.as_mut().unwrap().fade_in = Frame(200)),
            Rejection::Fades(_)
        ));
        assert!(matches!(
            refused(&|d| d.audio.as_mut().unwrap().volume_db = 20.0),
            Rejection::Volume(_)
        ));
        let outside = Rect {
            x: 1900,
            y: 0,
            width: 100,
            height: 100,
        };
        assert!(matches!(
            refused(&|d| d.video.as_mut().unwrap().crop = Some(outside)),
            Rejection::Crop(_)
        ));
    }

    #[test]
    fn a_draft_for_a_locked_track_is_refused() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 5 * SECOND));
        let draft = draft_of(&project, video);
        let audio_track = track(&project, TrackKind::Audio);
        lock(&mut project, audio_track);
        assert_eq!(
            apply(&mut project, vec![video, audio], draft).map(|_| ()),
            Err(Rejection::TrackLocked(audio_track))
        );
    }

    #[test]
    fn a_group_that_lost_a_clip_refuses_the_draft() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 5 * SECOND));
        let draft = draft_of(&project, video);
        remove_one(&project, audio).apply(&mut project).unwrap();
        assert_eq!(
            apply(&mut project, vec![video, audio], draft).map(|_| ()),
            Err(Rejection::GroupChanged)
        );
    }

    #[test]
    fn undo_restores_the_clips_as_they_were_when_the_draft_was_applied() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 5 * SECOND));
        let mut draft = draft_of(&project, video);
        draft.speed = 0.5;
        // Trimmed in the main window while the draft was made.
        trim(&mut project, video, Edge::End, 100).unwrap();
        let trimmed = project.clone();
        let mut command = apply(&mut project, vec![video, audio], draft).unwrap();
        assert_eq!(clip(&project, video).speed, 0.5);
        command.revert(&mut project);
        assert_eq!(project, trimmed);
    }
}
