//! Splitting a link group in two at a timeline frame.

use crate::command::{Rejection, clip_indices, group_indices};
use crate::model::{Clip, ClipEdits, ClipId, LinkId, Project, TrackId};
use crate::time::{Frame, MediaTime, Rational, frame_to_media, length_for};

/// Splits a clip and every clip linked to it at a frame inside them. The left parts keep
/// their ids and links; the right parts get new ids and, if the group was linked, a new link
/// of their own. Fades stay on the outer ends.
#[derive(Clone, Debug)]
pub struct SplitClips {
    clip: ClipId,
    at: Frame,
    /// The ids the right parts get, chosen on the first apply and kept for redoing.
    right: Vec<ClipId>,
    right_link: Option<LinkId>,
    before: Vec<(TrackId, Clip)>,
}

impl SplitClips {
    /// Splits `clip` and its partners at frame `at`.
    pub fn new(clip: ClipId, at: Frame) -> SplitClips {
        SplitClips {
            clip,
            at,
            right: Vec::new(),
            right_link: None,
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
        // Linked clips share their timing, so any one of them stands for all.
        let (t, i) = members[0];
        let first = &tracks[t].clips[i];
        if !(first.position < self.at && self.at < first.end()) {
            return Err(Rejection::SplitOutside(self.clip));
        }
        let rate = project.sequence.frame_rate;
        let source = split_time(first, self.at, rate).ok_or(Rejection::TooShort(self.clip))?;
        if self.right.len() != members.len() {
            let mut ids = project.fresh_ids();
            self.right = members.iter().map(|_| ids.clip()).collect();
            self.right_link = first.link.map(|_| ids.link());
        }
        self.before = members
            .iter()
            .map(|&(t, i)| (tracks[t].id, tracks[t].clips[i].clone()))
            .collect();
        let tracks = &mut project.sequence.tracks;
        for (&(t, i), &id) in members.iter().zip(&self.right) {
            let left = &mut tracks[t].clips[i];
            let mut right = left.clone();
            right.id = id;
            right.link = self.right_link;
            right.position = self.at;
            right.length = left.end() - self.at;
            right.source_in = source;
            left.length = self.at - left.position;
            left.source_out = source;
            // The cut gets no fade: fade-in stays on the left part, fade-out on the right.
            if let (ClipEdits::Audio(left), ClipEdits::Audio(right_edits)) =
                (&mut left.edits, &mut right.edits)
            {
                left.fade_out = Frame(0);
                right_edits.fade_in = Frame(0);
            }
            if let ClipEdits::Audio(edits) = &mut left.edits {
                edits.fade_in = edits.fade_in.min(self.at - left.position);
            }
            if let ClipEdits::Audio(edits) = &mut right.edits {
                edits.fade_out = edits.fade_out.min(right.length);
            }
            tracks[t].clips.insert(i + 1, right);
        }
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        for track in &mut project.sequence.tracks {
            track.clips.retain(|clip| !self.right.contains(&clip.id));
        }
        for (_, clip) in self.before.drain(..) {
            if let Ok((t, i)) = clip_indices(project, clip.id) {
                project.sequence.tracks[t].clips[i] = clip;
            }
        }
    }
}

/// The source time to cut `clip` at so that both parts keep the lengths the cut at frame `at`
/// gives them: near where the clip shows frame `at`, moved by at most a frame when rounding
/// would otherwise make the parts one frame longer or shorter than the whole.
fn split_time(clip: &Clip, at: Frame, rate: Rational) -> Option<MediaTime> {
    let (left, right) = (at - clip.position, clip.end() - at);
    let fits = |time: MediaTime| {
        time > clip.source_in
            && time < clip.source_out
            && length_for(time - clip.source_in, clip.speed, rate) == left
            && length_for(clip.source_out - time, clip.speed, rate) == right
    };
    let guess = clip.source_time_at(at, rate);
    let reach = (frame_to_media(Frame(1), rate).0 as f64 * clip.speed).ceil() as i64 + 1;
    std::iter::once(0)
        .chain((1..=reach).flat_map(|step| [step, -step]))
        .map(|step| guess + MediaTime(step))
        .find(|&time| fits(time))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::*;
    use crate::command::{Command, InsertClips, SetAudioEdits};
    use crate::model::{AudioEdits, ClipEdits, MediaId, TrackKind};
    use crate::time::{MediaTime, length_for};

    fn split(project: &mut Project, id: ClipId, at: i64) -> Result<Command, Rejection> {
        let mut command = Command::SplitClips(SplitClips::new(id, Frame(at)));
        command.apply(project).map(|()| command)
    }

    /// The clips on the track of kind `kind`, in order.
    fn on(project: &Project, kind: TrackKind) -> Vec<Clip> {
        let id = track(project, kind);
        project.sequence().track(id).unwrap().clips().to_vec()
    }

    #[test]
    fn a_linked_pair_splits_into_two_linked_pairs_and_reverts() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 30, (SECOND, 5 * SECOND)); // 30..150
        let before = project.clone();
        let mut command = split(&mut project, video, 90).unwrap();
        for kind in [TrackKind::Video, TrackKind::Audio] {
            let clips = on(&project, kind);
            assert_eq!(clips.len(), 2);
            let (left, right) = (&clips[0], &clips[1]);
            assert_eq!((left.position, left.length), (Frame(30), Frame(60)));
            assert_eq!((right.position, right.length), (Frame(90), Frame(60)));
            assert_eq!(left.source_in, MediaTime(SECOND));
            assert_eq!(left.source_out, MediaTime(3 * SECOND));
            assert_eq!(left.source_out, right.source_in);
            assert_eq!(right.source_out, MediaTime(5 * SECOND));
            assert!(left.id == video || left.id == audio);
            assert!(right.link.is_some() && right.link != left.link);
        }
        let right_video = on(&project, TrackKind::Video)[1].id;
        assert_eq!(project.link_group(right_video).len(), 2);
        assert_eq!(project.link_group(video).len(), 2);
        command.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn a_redo_gives_the_right_parts_the_same_ids() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 0, (0, 4 * SECOND));
        let mut command = split(&mut project, video, 60).unwrap();
        let first = on(&project, TrackKind::Video)[1].id;
        command.revert(&mut project);
        command.apply(&mut project).unwrap();
        assert_eq!(on(&project, TrackKind::Video)[1].id, first);
    }

    #[test]
    fn halves_add_up_exactly_at_any_speed() {
        for speed in [1.0, 2.0, 0.37] {
            let mut project = project();
            let video_track = track(&project, TrackKind::Video);
            // Odd source boundaries, so no frame lines up with a whole microsecond count.
            let mut whole = Clip::new(
                project.fresh_ids().clip(),
                MediaId(1),
                TrackKind::Video,
                (MediaTime(123_457), MediaTime(8_765_431)),
                Frame(7),
                fps30(),
            );
            whole.speed = speed;
            whole.length = length_for(whole.source_out - whole.source_in, speed, fps30());
            Command::InsertClips(InsertClips::new(vec![(video_track, whole.clone())]))
                .apply(&mut project)
                .unwrap();
            let at = whole.position.0 + whole.length.0 / 3;
            split(&mut project, whole.id, at).unwrap();
            let clips = on(&project, TrackKind::Video);
            assert_eq!(clips[0].length + clips[1].length, whole.length, "{speed}x");
            assert_eq!(clips[1].end(), whole.end(), "{speed}x");
            assert_eq!(clips[0].source_out, clips[1].source_in, "{speed}x");
            assert_eq!(clips[1].source_out, whole.source_out, "{speed}x");
        }
    }

    #[test]
    fn fades_stay_on_the_outer_ends() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 4 * SECOND)); // 120 frames
        let fades = AudioEdits {
            volume_db: -3.0,
            fade_in: Frame(20),
            fade_out: Frame(50),
        };
        Command::SetAudioEdits(SetAudioEdits::new(audio, fades))
            .apply(&mut project)
            .unwrap();
        split(&mut project, video, 40).unwrap();
        let clips = on(&project, TrackKind::Audio);
        let edits = |clip: &Clip| match &clip.edits {
            ClipEdits::Audio(edits) => edits.clone(),
            ClipEdits::Video(_) => unreachable!(),
        };
        let (left, right) = (edits(&clips[0]), edits(&clips[1]));
        assert_eq!((left.fade_in, left.fade_out), (Frame(20), Frame(0)));
        assert_eq!((right.fade_in, right.fade_out), (Frame(0), Frame(50)));
        assert_eq!((left.volume_db, right.volume_db), (-3.0, -3.0));
    }

    #[test]
    fn a_split_outside_the_clip_or_on_a_locked_track_is_refused() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 30, (0, 2 * SECOND)); // 30..90
        let before = project.clone();
        for at in [30, 90, 10, 200] {
            assert_eq!(
                split(&mut project, video, at).map(|_| ()),
                Err(Rejection::SplitOutside(video))
            );
            assert_eq!(project, before);
        }
        let audio_track = track(&project, TrackKind::Audio);
        lock(&mut project, audio_track);
        assert_eq!(
            split(&mut project, video, 60).map(|_| ()),
            Err(Rejection::TrackLocked(audio_track))
        );
    }
}
