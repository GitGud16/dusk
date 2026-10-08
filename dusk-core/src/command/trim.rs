//! Moving the start or the end of a clip and its linked partners.

use crate::command::{Notice, Rejection, group_indices, next_start, previous_end};
use crate::model::{Clip, ClipEdits, ClipId, MediaKind, Project, TrackId};
use crate::time::{Frame, MediaTime, length_for, source_span};

/// Which end of a clip a trim moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// The start: position and source in move together, the end stays.
    Start,
    /// The end: the source out moves, the start stays.
    End,
}

/// Moves one edge of a clip, and of every clip linked to it, to a timeline frame.
#[derive(Clone, Debug)]
pub struct TrimClips {
    clip: ClipId,
    edge: Edge,
    to: Frame,
    before: Vec<(TrackId, Clip)>,
    pub(super) notices: Vec<Notice>,
}

impl TrimClips {
    /// Moves `edge` of `clip` and its linked partners to frame `to`.
    pub fn new(clip: ClipId, edge: Edge, to: Frame) -> TrimClips {
        TrimClips {
            clip,
            edge,
            to,
            before: Vec::new(),
            notices: Vec::new(),
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        let mut notices = Vec::new();
        let members = group_indices(project, self.clip)?;
        for &(track, _) in &members {
            let track = &project.sequence.tracks[track];
            if track.locked {
                return Err(Rejection::TrackLocked(track.id));
            }
        }
        // Linked clips share source range, position, length and speed, so one stands for all.
        let (track, index) = members[0];
        let clip = &project.sequence.tracks[track].clips[index];
        let rate = project.sequence.frame_rate;
        let info = &project
            .media_ref(clip.media_id)
            .ok_or(Rejection::UnknownMedia(clip.media_id))?
            .info;
        let duration = info.duration;
        // A still has no source to run out of: it lasts as long as it is trimmed to.
        let still = info.kind == MediaKind::Still;
        let span = |length| source_span(length, clip.speed, rate);
        let length_of = |source| length_for(source, clip.speed, rate);

        let (source_in, source_out, position, length) = match self.edge {
            // The start stays: the source out follows from the new length.
            Edge::End => {
                let mut length = self.to - clip.position;
                if length < Frame(1) {
                    return Err(Rejection::TooShort(self.clip));
                }
                let mut at_edge = false;
                if !still && clip.source_in + span(length) > duration {
                    length = length_of(duration - clip.source_in);
                    at_edge = true;
                }
                let next = members
                    .iter()
                    .filter_map(|&(t, i)| next_start(project, t, i));
                if let Some(gap_end) = next.min()
                    && clip.position + length > gap_end
                {
                    length = gap_end - clip.position;
                    at_edge = false;
                    notices.push(Notice::CutAtGap(self.clip));
                }
                if at_edge {
                    notices.push(Notice::ReachedSourceEdge(self.clip));
                }
                let source_out = if still {
                    clip.source_out
                } else {
                    (clip.source_in + span(length)).min(duration)
                };
                (clip.source_in, source_out, clip.position, length)
            }
            // The end stays: the source in follows from the new length.
            Edge::Start => {
                let end = clip.end();
                let mut length = end - self.to;
                if length < Frame(1) {
                    return Err(Rejection::TooShort(self.clip));
                }
                if !still && clip.source_out - span(length) < MediaTime(0) {
                    length = length_of(clip.source_out);
                    notices.push(Notice::ReachedSourceEdge(self.clip));
                }
                let position = end - length;
                if position < Frame(0) {
                    return Err(Rejection::BeforeStart(self.clip));
                }
                for &(t, i) in &members {
                    if previous_end(project, t, i).is_some_and(|prev_end| position < prev_end) {
                        return Err(Rejection::Overlap(project.sequence.tracks[t].id));
                    }
                }
                let source_in = if still {
                    clip.source_in
                } else {
                    (clip.source_out - span(length)).max(MediaTime(0))
                };
                (source_in, clip.source_out, position, length)
            }
        };

        self.before = members
            .iter()
            .map(|&(t, i)| {
                let track = &project.sequence.tracks[t];
                (track.id, track.clips[i].clone())
            })
            .collect();
        self.notices = notices;
        for &(t, i) in &members {
            let clip = &mut project.sequence.tracks[t].clips[i];
            clip.source_in = source_in;
            clip.source_out = source_out;
            clip.position = position;
            clip.length = length;
            // Fades longer than the trimmed clip are shortened to fit, fade-in first.
            if let ClipEdits::Audio(edits) = &mut clip.edits {
                edits.fit_into(length);
            }
        }
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        for (track_id, before) in self.before.drain(..) {
            let clip = project
                .sequence
                .track_mut(track_id)
                .and_then(|track| track.clips.iter_mut().find(|c| c.id == before.id));
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
    use crate::command::{Command, InsertClips};
    use crate::model::{MediaId, TrackKind};

    #[test]
    fn trimming_the_end_shortens_every_linked_clip_and_reverts() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 10 * SECOND)); // 300 frames
        let before = project.clone();
        let mut command = trim(&mut project, audio, Edge::End, 150).unwrap();
        for id in [video, audio] {
            let clip = clip(&project, id);
            assert_eq!(
                (clip.length, clip.source_out),
                (Frame(150), MediaTime(5 * SECOND))
            );
            assert_eq!((clip.position, clip.source_in), (Frame(0), MediaTime(0)));
        }
        assert!(command.notices().is_empty());
        command.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn an_end_trim_into_the_next_clip_is_cut_at_the_gap() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 0, (0, 5 * SECOND)); // frames 0..150
        insert_pair(&mut project, 200, (6 * SECOND, 7 * SECOND));
        let command = trim(&mut project, video, Edge::End, 250).unwrap();
        let clip = clip(&project, video);
        assert_eq!(clip.end(), Frame(200));
        assert_eq!(clip.source_out, MediaTime(6_666_667));
        assert_eq!(command.notices(), [Notice::CutAtGap(video)]);
    }

    #[test]
    fn an_end_trim_stops_at_the_end_of_the_source() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 0, (0, 5 * SECOND));
        let command = trim(&mut project, video, Edge::End, 400).unwrap();
        let clip = clip(&project, video);
        assert_eq!(
            (clip.length, clip.source_out),
            (Frame(300), MediaTime(10 * SECOND))
        );
        assert_eq!(command.notices(), [Notice::ReachedSourceEdge(video)]);
    }

    #[test]
    fn trimming_the_start_moves_position_and_source_in_together() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 10 * SECOND));
        let before = project.clone();
        let mut command = trim(&mut project, video, Edge::Start, 30).unwrap();
        for id in [video, audio] {
            let clip = clip(&project, id);
            assert_eq!(
                (clip.position, clip.source_in),
                (Frame(30), MediaTime(SECOND))
            );
            assert_eq!((clip.length, clip.end()), (Frame(270), Frame(300)));
            assert_eq!(clip.source_out, MediaTime(10 * SECOND));
        }
        command.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn a_start_trim_stops_at_the_start_of_the_source() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 100, (SECOND, 5 * SECOND)); // 100..220
        let command = trim(&mut project, video, Edge::Start, 50).unwrap();
        let clip = clip(&project, video);
        assert_eq!((clip.source_in, clip.length), (MediaTime(0), Frame(150)));
        assert_eq!((clip.position, clip.end()), (Frame(70), Frame(220)));
        assert_eq!(command.notices(), [Notice::ReachedSourceEdge(video)]);
    }

    #[test]
    fn trims_that_break_an_invariant_are_refused() {
        let mut project = project();
        let (first, _) = insert_pair(&mut project, 0, (SECOND, 5 * SECOND)); // 0..120
        let (second, _) = insert_pair(&mut project, 120, (5 * SECOND, 9 * SECOND)); // 120..240
        let before = project.clone();
        let refused = |id, edge, to| {
            let mut project = before.clone();
            let result = trim(&mut project, id, edge, to).map(|_| ());
            assert_eq!(project, before, "a refused trim changed the project");
            result.unwrap_err()
        };
        let video_track = track(&before, TrackKind::Video);
        assert_eq!(
            refused(second, Edge::Start, 100),
            Rejection::Overlap(video_track)
        );
        assert_eq!(
            refused(first, Edge::Start, -10),
            Rejection::BeforeStart(first)
        );
        assert_eq!(refused(first, Edge::End, 0), Rejection::TooShort(first));
        assert_eq!(
            refused(second, Edge::Start, 240),
            Rejection::TooShort(second)
        );
        assert_eq!(
            refused(ClipId(999), Edge::End, 10),
            Rejection::UnknownClip(ClipId(999))
        );
    }

    #[test]
    fn a_refused_trim_reports_no_notices() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 10, (SECOND, 5 * SECOND)); // 10..130
        // Reaches the start of the source first, then would start before frame 0.
        let mut command = Command::TrimClips(TrimClips::new(video, Edge::Start, Frame(-50)));
        assert_eq!(
            command.apply(&mut project),
            Err(Rejection::BeforeStart(video))
        );
        assert!(command.notices().is_empty());
    }

    #[test]
    fn a_trim_that_touches_a_locked_track_is_refused() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 0, (0, 5 * SECOND));
        let audio_track = track(&project, TrackKind::Audio);
        let index = project
            .sequence
            .tracks
            .iter()
            .position(|track| track.id == audio_track)
            .unwrap();
        project.sequence.tracks[index].locked = true;
        assert_eq!(
            trim(&mut project, video, Edge::End, 100).map(|_| ()),
            Err(Rejection::TrackLocked(audio_track))
        );
    }

    #[test]
    fn a_still_trims_to_any_length_and_stops_at_the_next_clip() {
        let mut project = project();
        let photo = add_still(&mut project);
        let still = insert_still(&mut project, photo, 0, 150);
        insert_pair(&mut project, 900, (0, SECOND));
        let command = trim(&mut project, still, Edge::End, 600).unwrap();
        assert_eq!(clip(&project, still).length, Frame(600));
        assert!(command.notices().is_empty());
        let command = trim(&mut project, still, Edge::End, 1000).unwrap();
        assert_eq!(clip(&project, still).end(), Frame(900));
        assert_eq!(command.notices(), [Notice::CutAtGap(still)]);
        trim(&mut project, still, Edge::Start, 100).unwrap();
        let trimmed = clip(&project, still);
        assert_eq!((trimmed.position, trimmed.end()), (Frame(100), Frame(900)));
        assert_eq!(
            (trimmed.source_in, trimmed.source_out),
            (MediaTime(0), MediaTime(0))
        );
    }

    #[test]
    fn an_unlinked_clip_trims_alone() {
        let mut project = project();
        let video_track = track(&project, TrackKind::Video);
        let mut ids = project.fresh_ids();
        let clip_id = ids.clip();
        let alone = Clip::new(
            clip_id,
            MediaId(1),
            TrackKind::Video,
            (MediaTime(0), MediaTime(2 * SECOND)),
            Frame(0),
            fps30(),
        );
        Command::InsertClips(InsertClips::new(vec![(video_track, alone)]))
            .apply(&mut project)
            .unwrap();
        trim(&mut project, clip_id, Edge::End, 30).unwrap();
        assert_eq!(clip(&project, clip_id).length, Frame(30));
    }
}
