//! Edits as commands (docs/ARCHITECTURE.md, "Commands"). A command either changes the
//! project and keeps every timeline invariant, or leaves the project untouched and returns a
//! [`Rejection`] that says why. An applied command can be reverted exactly.

use std::fmt;

use crate::model::{Clip, ClipId, MediaId, MediaRef, Project, TrackId};
use crate::time::{Frame, MediaTime, length_for, source_span};

/// One edit of a project.
#[derive(Clone, Debug)]
pub enum Command {
    /// Adds a media file to the project.
    AddMedia(MediaRef),
    /// Places clips on tracks.
    InsertClips(InsertClips),
    /// Moves the start or the end of a clip and its linked partners.
    TrimClips(TrimClips),
    /// Several commands applied as one: all of them, or none.
    Batch(Vec<Command>),
}

impl Command {
    /// Applies the command, or leaves `project` untouched and says why not.
    pub fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        match self {
            Command::AddMedia(media) => {
                if project.media_ref(media.id).is_some() {
                    return Err(Rejection::DuplicateId);
                }
                project.media.push(media.clone());
                Ok(())
            }
            Command::InsertClips(insert) => insert.apply(project),
            Command::TrimClips(trim) => trim.apply(project),
            Command::Batch(commands) => {
                for applied in 0..commands.len() {
                    if let Err(rejection) = commands[applied].apply(project) {
                        for command in commands[..applied].iter_mut().rev() {
                            command.revert(project);
                        }
                        return Err(rejection);
                    }
                }
                Ok(())
            }
        }
    }

    /// Undoes the command, which must be the last one applied to `project`.
    pub fn revert(&mut self, project: &mut Project) {
        match self {
            Command::AddMedia(media) => project.media.retain(|other| other.id != media.id),
            Command::InsertClips(insert) => insert.revert(project),
            Command::TrimClips(trim) => trim.revert(project),
            Command::Batch(commands) => {
                for command in commands.iter_mut().rev() {
                    command.revert(project);
                }
            }
        }
    }

    /// What applying the command did beyond what was asked, for the UI to tell the user.
    pub fn notices(&self) -> Vec<Notice> {
        match self {
            Command::TrimClips(trim) => trim.notices.clone(),
            Command::Batch(commands) => commands.iter().flat_map(Command::notices).collect(),
            Command::AddMedia(_) | Command::InsertClips(_) => Vec::new(),
        }
    }
}

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

    fn apply(&self, project: &mut Project) -> Result<(), Rejection> {
        let tracks = self.check(project)?;
        for (index, (_, clip)) in tracks.into_iter().zip(&self.placements) {
            let clips = &mut project.sequence.tracks[index].clips;
            let at = clips.partition_point(|other| other.position < clip.position);
            clips.insert(at, clip.clone());
        }
        Ok(())
    }

    fn revert(&self, project: &mut Project) {
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
    notices: Vec<Notice>,
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

    fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
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
        let duration = project
            .media_ref(clip.media_id)
            .ok_or(Rejection::UnknownMedia(clip.media_id))?
            .info
            .duration;
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
                if clip.source_in + span(length) > duration {
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
                let source_out = (clip.source_in + span(length)).min(duration);
                (clip.source_in, source_out, clip.position, length)
            }
            // The end stays: the source in follows from the new length.
            Edge::Start => {
                let end = clip.end();
                let mut length = end - self.to;
                if length < Frame(1) {
                    return Err(Rejection::TooShort(self.clip));
                }
                if clip.source_out - span(length) < MediaTime(0) {
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
                let source_in = (clip.source_out - span(length)).max(MediaTime(0));
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
        }
        Ok(())
    }

    fn revert(&mut self, project: &mut Project) {
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

/// The track and clip indices of `id` and every clip linked to it.
fn group_indices(project: &Project, id: ClipId) -> Result<Vec<(usize, usize)>, Rejection> {
    let group = project.link_group(id);
    if group.is_empty() {
        return Err(Rejection::UnknownClip(id));
    }
    let mut indices = Vec::with_capacity(group.len());
    for (t, track) in project.sequence.tracks.iter().enumerate() {
        for (i, clip) in track.clips.iter().enumerate() {
            if group.contains(&clip.id) {
                indices.push((t, i));
            }
        }
    }
    Ok(indices)
}

/// Where the clip after clip `i` of track `t` starts, if there is one.
fn next_start(project: &Project, t: usize, i: usize) -> Option<Frame> {
    project.sequence.tracks[t]
        .clips
        .get(i + 1)
        .map(|clip| clip.position)
}

/// Where the clip before clip `i` of track `t` ends, if there is one.
fn previous_end(project: &Project, t: usize, i: usize) -> Option<Frame> {
    let previous = i.checked_sub(1)?;
    project.sequence.tracks[t]
        .clips
        .get(previous)
        .map(Clip::end)
}

/// The checks every clip passes, wherever it is on the timeline.
fn check_clip(project: &Project, clip: &Clip) -> Result<(), Rejection> {
    let media = project
        .media_ref(clip.media_id)
        .ok_or(Rejection::UnknownMedia(clip.media_id))?;
    if clip.source_in < MediaTime(0)
        || clip.source_in >= clip.source_out
        || clip.source_out > media.info.duration
    {
        return Err(Rejection::SourceRange(clip.id));
    }
    if !(MIN_SPEED..=MAX_SPEED).contains(&clip.speed) {
        return Err(Rejection::Speed(clip.id));
    }
    let rate = project.sequence.frame_rate;
    if clip.length != length_for(clip.source_out - clip.source_in, clip.speed, rate) {
        return Err(Rejection::Length(clip.id));
    }
    if clip.position < Frame(0) {
        return Err(Rejection::BeforeStart(clip.id));
    }
    Ok(())
}

fn overlaps(a: &Clip, b: &Clip) -> bool {
    a.position < b.end() && b.position < a.end()
}

fn same_timing(a: &Clip, b: &Clip) -> bool {
    (a.source_in, a.source_out, a.position, a.length)
        == (b.source_in, b.source_out, b.position, b.length)
        && a.speed == b.speed
}

/// The slowest and fastest clip speeds (docs/ARCHITECTURE.md, "Core data model").
const MIN_SPEED: f64 = 0.1;
const MAX_SPEED: f64 = 32.0;

/// Why a command was refused. The project is unchanged.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    /// The command names a track the sequence does not have.
    #[error("that track no longer exists; reopen the view and try again")]
    UnknownTrack(TrackId),
    /// The command names a clip the project does not have.
    #[error("that clip no longer exists; reopen the view and try again")]
    UnknownClip(ClipId),
    /// A clip refers to a media file the project does not have.
    #[error("the clip's media file is not in the project; import it first")]
    UnknownMedia(MediaId),
    /// A new media file or clip reuses an id.
    #[error("an item with the same id is already in the project")]
    DuplicateId,
    /// The edit would change a clip on a locked track.
    #[error("a track this edit touches is locked; unlock the track or unlink the clips")]
    TrackLocked(TrackId),
    /// A video clip on an audio track, or the other way round.
    #[error("video clips go on video tracks and audio clips on audio tracks")]
    WrongTrackKind(ClipId),
    /// The edit would make two clips on a track overlap.
    #[error("the clip would overlap another clip; make room first")]
    Overlap(TrackId),
    /// A clip's source range does not lie within its media file.
    #[error("the clip reaches outside its source file")]
    SourceRange(ClipId),
    /// The edit would leave a clip shorter than one frame.
    #[error("a clip must be at least one frame long")]
    TooShort(ClipId),
    /// The edit would move a clip before the start of the timeline.
    #[error("a clip cannot start before the beginning of the timeline")]
    BeforeStart(ClipId),
    /// A clip's speed is outside 0.1x to 32x.
    #[error("speed must be between 0.1x and 32x")]
    Speed(ClipId),
    /// A clip's length does not follow from its source range and speed.
    #[error("the clip's length does not match its source range and speed")]
    Length(ClipId),
    /// Linked clips differ in source range, position, length or speed.
    #[error("linked clips must share their source range, position, length and speed")]
    LinkMismatch(ClipId),
}

/// Something a command did beyond what was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// An end trim ran into the next clip, so the clip was cut where that clip starts.
    CutAtGap(ClipId),
    /// A trim reached the start or the end of the source file and stopped there.
    ReachedSourceEdge(ClipId),
}

impl fmt::Display for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Notice::CutAtGap(_) => {
                "The clip was cut where the next clip starts; ripple the timeline to make room."
            }
            Notice::ReachedSourceEdge(_) => "The clip reached the edge of its source file.",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioEdits, ClipEdits, LinkId, MediaInfo, MediaKind, TrackKind};
    use crate::time::{MediaTime, Rational};

    const SECOND: i64 = 1_000_000;

    fn fps30() -> Rational {
        Rational::new(30, 1).unwrap()
    }

    fn track(project: &Project, kind: TrackKind) -> TrackId {
        project
            .sequence()
            .tracks()
            .iter()
            .find(|track| track.kind() == kind)
            .unwrap()
            .id()
    }

    /// A project at 30 fps holding one 10 s video file with sound, MediaId(1).
    fn project() -> Project {
        let mut project = Project::new(fps30(), (1920, 1080));
        let media = MediaRef {
            id: MediaId(1),
            path: "clip.mp4".into(),
            info: MediaInfo {
                kind: MediaKind::Video,
                duration: MediaTime(10 * SECOND),
                has_video: true,
                has_audio: true,
                frame_rate: Some(fps30()),
                vfr: false,
                width: 1920,
                height: 1080,
            },
        };
        Command::AddMedia(media).apply(&mut project).unwrap();
        project
    }

    /// Inserts a linked video and audio clip of MediaId(1) and returns their ids.
    fn insert_pair(project: &mut Project, position: i64, source: (i64, i64)) -> (ClipId, ClipId) {
        let mut ids = project.fresh_ids();
        let link = ids.link();
        let mut placements = Vec::new();
        let mut clip_ids = Vec::new();
        for kind in [TrackKind::Video, TrackKind::Audio] {
            let mut clip = Clip::new(
                ids.clip(),
                MediaId(1),
                kind,
                (MediaTime(source.0), MediaTime(source.1)),
                Frame(position),
                fps30(),
            );
            clip.link = Some(link);
            clip_ids.push(clip.id);
            placements.push((track(project, kind), clip));
        }
        Command::InsertClips(InsertClips::new(placements))
            .apply(project)
            .unwrap();
        (clip_ids[0], clip_ids[1])
    }

    fn clip(project: &Project, id: ClipId) -> Clip {
        project.find_clip(id).unwrap().1.clone()
    }

    fn trim(project: &mut Project, id: ClipId, edge: Edge, to: i64) -> Result<Command, Rejection> {
        let mut command = Command::TrimClips(TrimClips::new(id, edge, Frame(to)));
        command.apply(project).map(|()| command)
    }

    #[test]
    fn adding_media_lists_it_once_and_reverts() {
        let empty = Project::new(fps30(), (1920, 1080));
        let mut project = empty.clone();
        let media = project_media();
        let mut add = Command::AddMedia(media.clone());
        add.apply(&mut project).unwrap();
        assert_eq!(project.media(), std::slice::from_ref(&media));
        assert_eq!(
            Command::AddMedia(media).apply(&mut project),
            Err(Rejection::DuplicateId)
        );
        add.revert(&mut project);
        assert_eq!(project, empty);
    }

    fn project_media() -> MediaRef {
        project().media()[0].clone()
    }

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

    #[test]
    fn a_batch_applies_all_or_nothing_and_reverts_in_reverse() {
        let empty = Project::new(fps30(), (1920, 1080));
        let media = project_media();
        let video_track = track(&empty, TrackKind::Video);
        let clip = Clip::new(
            ClipId(1),
            MediaId(1),
            TrackKind::Video,
            (MediaTime(0), MediaTime(SECOND)),
            Frame(0),
            fps30(),
        );
        let mut bad = clip.clone();
        bad.length = Frame(2);

        let mut project = empty.clone();
        let mut refused = Command::Batch(vec![
            Command::AddMedia(media.clone()),
            Command::InsertClips(InsertClips::new(vec![(video_track, bad)])),
        ]);
        assert_eq!(
            refused.apply(&mut project),
            Err(Rejection::Length(ClipId(1)))
        );
        assert_eq!(project, empty);

        let mut import = Command::Batch(vec![
            Command::AddMedia(media),
            Command::InsertClips(InsertClips::new(vec![(video_track, clip)])),
        ]);
        import.apply(&mut project).unwrap();
        assert_eq!(project.sequence().tracks()[0].clips().len(), 1);
        import.revert(&mut project);
        assert_eq!(project, empty);
    }

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
        project.sequence.tracks[1].locked = true;
        assert_eq!(
            trim(&mut project, video, Edge::End, 100).map(|_| ()),
            Err(Rejection::TrackLocked(audio_track))
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
