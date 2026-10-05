//! Fixtures shared by the command tests.

use crate::command::{Command, Edge, InsertClips, Rejection, TrimClips};
use crate::model::{
    Clip, ClipId, MediaId, MediaInfo, MediaKind, MediaRef, Project, TrackId, TrackKind,
};
use crate::time::{Frame, MediaTime, Rational};

pub(crate) const SECOND: i64 = 1_000_000;

/// Locks `track` directly, as a fixture.
pub(crate) fn lock(project: &mut Project, track: TrackId) {
    let index = project
        .sequence
        .tracks
        .iter()
        .position(|candidate| candidate.id == track)
        .unwrap();
    project.sequence.tracks[index].locked = true;
}

pub(crate) fn fps30() -> Rational {
    Rational::new(30, 1).unwrap()
}

pub(crate) fn track(project: &Project, kind: TrackKind) -> TrackId {
    project
        .sequence()
        .tracks()
        .iter()
        .find(|track| track.kind() == kind)
        .unwrap()
        .id()
}

/// A project at 30 fps holding one 10 s video file with sound, MediaId(1).
pub(crate) fn project() -> Project {
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

/// Adds a 4032x3024 photo to `project` and returns its id.
pub(crate) fn add_still(project: &mut Project) -> MediaId {
    let id = project.fresh_ids().media();
    let media = MediaRef {
        id,
        path: "photo.jpg".into(),
        info: MediaInfo {
            kind: MediaKind::Still,
            duration: MediaTime(0),
            has_video: true,
            has_audio: false,
            frame_rate: None,
            vfr: false,
            width: 4032,
            height: 3024,
        },
    };
    Command::AddMedia(media).apply(project).unwrap();
    id
}

/// Puts a still clip of `media` on V1 from `position` for `length` frames and returns its id.
pub(crate) fn insert_still(
    project: &mut Project,
    media: MediaId,
    position: i64,
    length: i64,
) -> ClipId {
    let clip = Clip::still(
        project.fresh_ids().clip(),
        media,
        Frame(position),
        Frame(length),
    );
    let id = clip.id;
    let video = track(project, TrackKind::Video);
    Command::InsertClips(InsertClips::new(vec![(video, clip)]))
        .apply(project)
        .unwrap();
    id
}

/// Inserts a linked video and audio clip of MediaId(1) and returns their ids.
pub(crate) fn insert_pair(
    project: &mut Project,
    position: i64,
    source: (i64, i64),
) -> (ClipId, ClipId) {
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

pub(crate) fn clip(project: &Project, id: ClipId) -> Clip {
    project.find_clip(id).unwrap().1.clone()
}

pub(crate) fn trim(
    project: &mut Project,
    id: ClipId,
    edge: Edge,
    to: i64,
) -> Result<Command, Rejection> {
    let mut command = Command::TrimClips(TrimClips::new(id, edge, Frame(to)));
    command.apply(project).map(|()| command)
}
