//! Changing the sequence's frame rate and size (docs/ARCHITECTURE.md, "Sequence settings").

use std::collections::HashMap;

use crate::command::{Notice, Rejection};
use crate::model::{ClipEdits, ClipId, LinkId, MediaId, MediaKind, Project, Track};
use crate::time::{
    Frame, MediaTime, Rational, frame_to_media, length_for, media_to_frame, source_span,
};

/// The smallest and largest sequence side, in pixels.
pub const SEQUENCE_SIDES: std::ops::RangeInclusive<u32> = 16..=8192;

/// Sets the sequence's frame rate and picture size. A new rate needs every track unlocked:
/// every clip's start and end are converted to time and rounded to the new rate, so abutting
/// clips stay abutting; fades convert as durations, and source ends follow the new lengths.
/// A clip that would round to no frames keeps one, and every clip that starts at or after its
/// end moves right by a frame.
#[derive(Clone, Debug)]
pub struct SetSequenceSettings {
    rate: Rational,
    resolution: (u32, u32),
    before: Option<(Rational, (u32, u32), Vec<Track>)>,
    pub(super) notices: Vec<Notice>,
}

impl SetSequenceSettings {
    /// Gives the sequence frame rate `rate` and picture size `resolution` (width, height).
    pub fn new(rate: Rational, resolution: (u32, u32)) -> SetSequenceSettings {
        SetSequenceSettings {
            rate,
            resolution,
            before: None,
            notices: Vec::new(),
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        let (width, height) = self.resolution;
        if !SEQUENCE_SIDES.contains(&width) || !SEQUENCE_SIDES.contains(&height) {
            return Err(Rejection::Resolution);
        }
        let sequence = &project.sequence;
        let old = sequence.frame_rate;
        let mut tracks = sequence.tracks.clone();
        let mut notices = Vec::new();
        if old != self.rate {
            if let Some(locked) = sequence.tracks.iter().find(|track| track.locked) {
                return Err(Rejection::TrackLocked(locked.id));
            }
            // What each clip's source holds: its duration, or `None` for a still.
            let sources: HashMap<MediaId, Option<MediaTime>> = project
                .media
                .iter()
                .map(|media| {
                    let still = media.info.kind == MediaKind::Still;
                    (media.id, (!still).then_some(media.info.duration))
                })
                .collect();
            notices = convert(&mut tracks, old, self.rate, &sources);
        }
        self.before = Some((old, sequence.resolution, sequence.tracks.clone()));
        self.notices = notices;
        let sequence = &mut project.sequence;
        sequence.frame_rate = self.rate;
        sequence.resolution = self.resolution;
        sequence.tracks = tracks;
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        if let Some((rate, resolution, tracks)) = self.before.take() {
            let sequence = &mut project.sequence;
            sequence.frame_rate = rate;
            sequence.resolution = resolution;
            sequence.tracks = tracks;
        }
    }
}

/// Converts every clip in `tracks` from frame rate `old` to `new` and returns what to tell
/// the user.
fn convert(
    tracks: &mut [Track],
    old: Rational,
    new: Rational,
    sources: &HashMap<MediaId, Option<MediaTime>>,
) -> Vec<Notice> {
    let at_new = |frame: Frame| media_to_frame(frame_to_media(frame, old), new);
    // The link group (or lone clip) of every clip that would vanish, once each.
    let mut vanished: Vec<(ClipId, Option<LinkId>)> = Vec::new();
    for clip in tracks.iter_mut().flat_map(|track| track.clips.iter_mut()) {
        let start = at_new(clip.position);
        let end = at_new(clip.end());
        clip.position = start;
        clip.length = end - start;
        if clip.length < Frame(1) {
            clip.length = Frame(1);
            let known = clip
                .link
                .is_some_and(|link| vanished.iter().any(|(_, other)| *other == Some(link)));
            if !known {
                vanished.push((clip.id, clip.link));
            }
        }
        if let ClipEdits::Audio(edits) = &mut clip.edits {
            (edits.fade_in, edits.fade_out) = (at_new(edits.fade_in), at_new(edits.fade_out));
            edits.fit_into(clip.length);
        }
    }
    // Each clip that kept a frame pushes every clip that started where it ended, on every
    // track, one frame right; its link partners stay level with it.
    let mut notices = Vec::new();
    for (id, link) in vanished {
        let in_group =
            |clip: &crate::model::Clip| clip.id == id || (link.is_some() && clip.link == link);
        let Some(at) = tracks
            .iter()
            .flat_map(|track| &track.clips)
            .find(|clip| clip.id == id)
            .map(|clip| clip.position)
        else {
            continue;
        };
        for clip in tracks.iter_mut().flat_map(|track| track.clips.iter_mut()) {
            if !in_group(clip) && clip.position >= at {
                clip.position = clip.position + Frame(1);
            }
        }
        notices.push(Notice::LengthenedToOneFrame(id));
    }
    // Source ends follow the new lengths, within the source.
    for clip in tracks.iter_mut().flat_map(|track| track.clips.iter_mut()) {
        let Some(Some(duration)) = sources.get(&clip.media_id) else {
            continue;
        };
        if length_for(clip.source_out - clip.source_in, clip.speed, new) != clip.length {
            let out = clip.source_in + source_span(clip.length, clip.speed, new);
            clip.source_out = out.min(*duration);
            clip.length = length_for(clip.source_out - clip.source_in, clip.speed, new);
        }
    }
    notices
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::*;
    use crate::command::{Command, InsertClips, SetAudioEdits};
    use crate::model::{AudioEdits, Clip, ClipEdits, ClipId, MediaId, TrackKind};
    use crate::time::{Frame, MediaTime, length_for};

    fn rate(num: u32, den: u32) -> Rational {
        Rational::new(num, den).unwrap()
    }

    fn settle(project: &mut Project, to: Rational) -> Result<Command, Rejection> {
        let resolution = project.sequence().resolution();
        let mut command = Command::SetSequenceSettings(SetSequenceSettings::new(to, resolution));
        command.apply(project).map(|()| command)
    }

    /// Every clip still keeps its length rule at the sequence's rate.
    fn assert_lengths_follow_sources(project: &Project) {
        let rate = project.sequence().frame_rate();
        for track in project.sequence().tracks() {
            for clip in track.clips() {
                assert_eq!(
                    clip.length,
                    length_for(clip.source_out - clip.source_in, clip.speed, rate),
                    "{:?}",
                    clip.id
                );
            }
        }
    }

    #[test]
    fn a_new_size_alone_changes_no_clip_and_ignores_locks() {
        let mut project = project();
        insert_pair(&mut project, 0, (0, SECOND));
        let video = track(&project, TrackKind::Video);
        lock(&mut project, video);
        let before = project.clone();
        let mut command =
            Command::SetSequenceSettings(SetSequenceSettings::new(fps30(), (1080, 1920)));
        command.apply(&mut project).unwrap();
        assert_eq!(project.sequence().resolution(), (1080, 1920));
        assert_eq!(project.sequence().tracks(), before.sequence().tracks());
        command.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn a_new_rate_converts_clips_through_time_and_reverts() {
        let mut project = project();
        let (first, _) = insert_pair(&mut project, 30, (0, 2 * SECOND)); // 30..90
        let (second, _) = insert_pair(&mut project, 90, (5 * SECOND, 6 * SECOND)); // 90..120
        let before = project.clone();
        let mut command = settle(&mut project, rate(25, 1)).unwrap();
        let (first, second) = (clip(&project, first), clip(&project, second));
        assert_eq!((first.position, first.end()), (Frame(25), Frame(75)));
        assert_eq!((second.position, second.end()), (Frame(75), Frame(100)));
        assert_lengths_follow_sources(&project);
        assert_eq!(project.sequence().frame_rate(), rate(25, 1));
        command.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn abutting_clips_stay_abutting_at_an_awkward_rate() {
        let mut project = project();
        let (first, _) = insert_pair(&mut project, 7, (123_457, 3_456_789));
        let end = clip(&project, first).end().0;
        let (second, _) = insert_pair(&mut project, end, (4 * SECOND, 7 * SECOND));
        settle(&mut project, rate(24000, 1001)).unwrap();
        assert_eq!(clip(&project, first).end(), clip(&project, second).position);
        assert_lengths_follow_sources(&project);
    }

    #[test]
    fn fades_convert_as_durations() {
        let mut project = project();
        let (_, audio) = insert_pair(&mut project, 0, (0, 4 * SECOND)); // 120 frames
        let fades = AudioEdits {
            volume_db: 0.0,
            fade_in: Frame(30),
            fade_out: Frame(15),
        };
        Command::SetAudioEdits(SetAudioEdits::new(audio, fades))
            .apply(&mut project)
            .unwrap();
        settle(&mut project, rate(60, 1)).unwrap();
        let ClipEdits::Audio(edits) = clip(&project, audio).edits else {
            unreachable!()
        };
        assert_eq!((edits.fade_in, edits.fade_out), (Frame(60), Frame(30)));
    }

    #[test]
    fn a_clip_that_would_vanish_keeps_a_frame_and_pushes_later_clips() {
        let mut project = project();
        let rate60 = rate(60, 1);
        Command::SetSequenceSettings(SetSequenceSettings::new(rate60, (1920, 1080)))
            .apply(&mut project)
            .unwrap();
        // One frame at 60 fps (16 667 µs), between two clips.
        let video = track(&project, TrackKind::Video);
        let make = |id, source: (i64, i64), position| {
            let mut clip = Clip::new(
                ClipId(id),
                MediaId(1),
                TrackKind::Video,
                (MediaTime(source.0), MediaTime(source.1)),
                Frame(position),
                rate60,
            );
            clip.link = None;
            clip
        };
        // Frame 64 at 60 fps, 1 066 667..1 083 333 µs: both ends round to frame 26 at 24 fps.
        let tiny = make(1, (0, 16_667), 64);
        let after = make(2, (SECOND, 2 * SECOND), 65); // 65..125
        Command::InsertClips(InsertClips::new(vec![(video, tiny), (video, after)]))
            .apply(&mut project)
            .unwrap();
        let command = settle(&mut project, rate(24, 1)).unwrap();
        let (tiny, after) = (clip(&project, ClipId(1)), clip(&project, ClipId(2)));
        assert_eq!(tiny.length, Frame(1));
        assert!(after.position >= tiny.end());
        assert_eq!(command.notices(), [Notice::LengthenedToOneFrame(ClipId(1))]);
        assert_lengths_follow_sources(&project);
    }

    #[test]
    fn a_new_rate_needs_every_track_unlocked() {
        let mut project = project();
        let audio = track(&project, TrackKind::Audio);
        lock(&mut project, audio);
        assert_eq!(
            settle(&mut project, rate(25, 1)).map(|_| ()),
            Err(Rejection::TrackLocked(audio))
        );
    }

    #[test]
    fn a_size_out_of_range_is_refused() {
        let mut project = project();
        for size in [(8, 1080), (1920, 0), (9000, 1080)] {
            assert_eq!(
                Command::SetSequenceSettings(SetSequenceSettings::new(fps30(), size))
                    .apply(&mut project),
                Err(Rejection::Resolution)
            );
        }
    }
}
