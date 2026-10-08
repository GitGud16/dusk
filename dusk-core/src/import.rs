//! Bringing a media file into the project (docs/FEATURES.md, "Import").

use std::path::PathBuf;

use crate::command::{Command, InsertClips, Rejection, nearest_free_position};
use crate::model::{Clip, MediaId, MediaInfo, MediaKind, MediaRef, Project, TrackId, TrackKind};
use crate::time::{Frame, MediaTime, media_to_frame};

/// How long a still image lasts when it is placed (docs/FEATURES.md, "Import").
pub const STILL_LENGTH: MediaTime = MediaTime(5_000_000);

/// The command that adds the file at `path` to `project` under a new id, which it returns.
pub fn add_media(project: &Project, path: PathBuf, info: MediaInfo) -> (MediaId, Command) {
    let id = project.fresh_ids().media();
    (id, Command::AddMedia(MediaRef { id, path, info }))
}

/// The command that places media `media` at `position`: its picture on track `video` and
/// its sound on track `audio`, linked when it has both, each showing the whole file; a still
/// image lasts five seconds. Applying it can still be refused, for example when the clips
/// would overlap others.
pub fn place(
    project: &Project,
    media: MediaId,
    position: Frame,
    video: TrackId,
    audio: TrackId,
) -> Result<Command, Rejection> {
    let media = project
        .media_ref(media)
        .ok_or(Rejection::UnknownMedia(media))?;
    let placements = placements(project, media, position, video, audio);
    Ok(Command::InsertClips(InsertClips::new(placements)))
}

/// The command that places media `media` at `position` on the first pair of tracks where it
/// fits: V1 and A1, then V2 and A2, and so on. When it fits nowhere, the reason it does not
/// fit on the first pair.
pub fn place_where_free(
    project: &Project,
    media: MediaId,
    position: Frame,
) -> Result<Command, Rejection> {
    let tracks_of = |kind| -> Vec<TrackId> {
        let tracks = project.sequence().tracks().iter();
        tracks
            .filter(|track| track.kind() == kind)
            .map(|track| track.id())
            .collect()
    };
    let (video, audio) = (tracks_of(TrackKind::Video), tracks_of(TrackKind::Audio));
    // The i-th track of a kind, or its last one when it has fewer.
    let pick = |tracks: &[TrackId], i: usize| {
        tracks
            .get(i)
            .or(tracks.last())
            .copied()
            .unwrap_or(TrackId(0))
    };
    let mut first_refusal = None;
    for i in 0..video.len().max(audio.len()).max(1) {
        let (v, a) = (pick(&video, i), pick(&audio, i));
        let mut trial = project.clone();
        match place(project, media, position, v, a)?.apply(&mut trial) {
            Ok(()) => return place(project, media, position, v, a),
            Err(rejection) => {
                first_refusal.get_or_insert(rejection);
            }
        }
    }
    Err(first_refusal.unwrap_or(Rejection::UnknownMedia(media)))
}

/// Where media `media`, placed by [`place`] on tracks `video` and `audio`, could start nearest
/// to frame `wanted` without overlapping anything; `None` when it cannot go on those tracks at
/// all, for example because one is locked.
pub fn nearest_free_place(
    project: &Project,
    media: MediaId,
    wanted: Frame,
    video: TrackId,
    audio: TrackId,
) -> Option<Frame> {
    // Placed after everything, where it always fits, the new clips can be asked where they
    // would fit nearest to `wanted`.
    let mut trial = project.clone();
    place(project, media, project.sequence().end(), video, audio)
        .ok()?
        .apply(&mut trial)
        .ok()?;
    let first = project.fresh_ids().clip();
    nearest_free_position(&trial, first, wanted, None)
}

/// Where media `media` dragged onto tracks `video` and `audio` starts when let go at frame
/// `wanted`: at the beginning while the timeline is empty, so a first clip has no black before
/// it, and otherwise the nearest place it fits ([`nearest_free_place`]). `None` where it cannot
/// go on those tracks.
pub fn drop_place(
    project: &Project,
    media: MediaId,
    wanted: Frame,
    video: TrackId,
    audio: TrackId,
) -> Option<Frame> {
    let empty = project.sequence().end() == Frame(0);
    let wanted = if empty { Frame(0) } else { wanted };
    nearest_free_place(project, media, wanted, video, audio)
}

/// The clips that show all of `media` from `position`, on track `video` and track `audio`.
fn placements(
    project: &Project,
    media: &MediaRef,
    position: Frame,
    video: TrackId,
    audio: TrackId,
) -> Vec<(TrackId, Clip)> {
    let mut ids = project.fresh_ids();
    let rate = project.sequence().frame_rate();
    if media.info.kind == MediaKind::Still {
        let length = media_to_frame(STILL_LENGTH, rate);
        return vec![(video, Clip::still(ids.clip(), media.id, position, length))];
    }
    let link = (media.info.has_video && media.info.has_audio).then(|| ids.link());
    let source = (MediaTime(0), media.info.duration);
    let streams = [
        (media.info.has_video, TrackKind::Video, video),
        (media.info.has_audio, TrackKind::Audio, audio),
    ];
    streams
        .into_iter()
        .filter(|(wanted, _, _)| *wanted)
        .map(|(_, kind, track)| {
            let mut clip = Clip::new(ids.clip(), media.id, kind, source, position, rate);
            clip.link = link;
            (track, clip)
        })
        .collect()
}

/// The command that adds the file at `path` to `project` under a new id and places it at
/// `position`: a video clip on the first video track and an audio clip on the first audio
/// track, linked when the file has both, each showing the whole file. Applying it can still be
/// refused, for example when the clips would overlap others.
pub fn import(project: &Project, path: PathBuf, info: MediaInfo, position: Frame) -> Command {
    let (id, add) = add_media(project, path.clone(), info.clone());
    let media = MediaRef { id, path, info };
    let first = |kind| {
        let tracks = project.sequence().tracks().iter();
        tracks
            .filter(|track| track.kind() == kind)
            .map(|track| track.id())
            .next()
    };
    let (Some(video), Some(audio)) = (first(TrackKind::Video), first(TrackKind::Audio)) else {
        return add;
    };
    let placements = placements(project, &media, position, video, audio);
    Command::Batch(vec![
        add,
        Command::InsertClips(InsertClips::new(placements)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MediaId, MediaKind, TrackId};
    use crate::time::Rational;

    fn fps30() -> Rational {
        Rational::new(30, 1).unwrap()
    }

    fn info(has_video: bool, has_audio: bool) -> MediaInfo {
        MediaInfo {
            kind: if has_video {
                MediaKind::Video
            } else {
                MediaKind::Audio
            },
            duration: MediaTime(2_000_000),
            has_video,
            has_audio,
            frame_rate: has_video.then(fps30),
            vfr: false,
            width: 1920,
            height: 1080,
            orientation: crate::orientation::Orientation::UPRIGHT,
        }
    }

    #[test]
    fn a_file_with_video_and_audio_lands_as_two_linked_clips() {
        let mut project = Project::new(fps30(), (1920, 1080));
        import(&project, "clip.mp4".into(), info(true, true), Frame(15))
            .apply(&mut project)
            .unwrap();

        assert_eq!(project.media().len(), 1);
        assert_eq!(project.media()[0].id, MediaId(1));
        // V1 and A1, the first track of each kind.
        let tracks = project.sequence().tracks();
        let video = &tracks[0].clips()[0];
        let audio = &tracks[2].clips()[0];
        assert_eq!(
            (tracks[0].kind(), tracks[2].kind()),
            (TrackKind::Video, TrackKind::Audio)
        );
        assert!(tracks[1].clips().is_empty() && tracks[3].clips().is_empty());
        assert_eq!(
            (video.kind(), audio.kind()),
            (TrackKind::Video, TrackKind::Audio)
        );
        assert!(video.link.is_some() && video.link == audio.link);
        for clip in [video, audio] {
            assert_eq!((clip.position, clip.length), (Frame(15), Frame(60)));
            assert_eq!(
                (clip.source_in, clip.source_out),
                (MediaTime(0), MediaTime(2_000_000))
            );
            assert_eq!(clip.media_id, MediaId(1));
        }
        assert_ne!(video.id, audio.id);
    }

    #[test]
    fn an_audio_file_lands_as_one_audio_clip() {
        let mut project = Project::new(fps30(), (1920, 1080));
        import(&project, "clip.mp4".into(), info(false, true), Frame(0))
            .apply(&mut project)
            .unwrap();
        let tracks = project.sequence().tracks();
        assert!(tracks[0].clips().is_empty());
        assert_eq!(tracks[2].clips().len(), 1);
        assert_eq!(tracks[2].clips()[0].link, None);
    }

    #[test]
    fn a_photo_lands_as_one_five_second_video_clip() {
        let mut project = Project::new(fps30(), (1920, 1080));
        let photo = MediaInfo {
            kind: MediaKind::Still,
            duration: MediaTime(0),
            has_video: true,
            has_audio: false,
            frame_rate: None,
            vfr: false,
            width: 4032,
            height: 3024,
            orientation: crate::orientation::Orientation::UPRIGHT,
        };
        import(&project, "photo.heic".into(), photo, Frame(30))
            .apply(&mut project)
            .unwrap();
        let tracks = project.sequence().tracks();
        let clips = tracks[0].clips();
        assert_eq!(clips.len(), 1);
        assert_eq!(
            (clips[0].position, clips[0].length),
            (Frame(30), Frame(150))
        );
        assert_eq!(clips[0].link, None);
        assert!(tracks[2].clips().is_empty());
    }

    #[test]
    fn media_is_placed_on_the_tracks_asked_for_as_often_as_wanted() {
        let mut project = Project::new(fps30(), (1920, 1080));
        let (media, add) = add_media(&project, "clip.mp4".into(), info(true, true));
        let mut add = add;
        add.apply(&mut project).unwrap();
        let tracks: Vec<TrackId> = project.sequence().tracks().iter().map(|t| t.id()).collect();
        let (v2, a2) = (tracks[1], tracks[3]);
        let before = project.clone();
        let mut first = place(&project, media, Frame(0), v2, a2).unwrap();
        first.apply(&mut project).unwrap();
        place(&project, media, Frame(100), v2, a2)
            .unwrap()
            .apply(&mut project)
            .unwrap();
        assert_eq!(
            place(&project, MediaId(77), Frame(0), v2, a2).map(|_| ()),
            Err(Rejection::UnknownMedia(MediaId(77)))
        );
        let on_v2 = project.sequence().track(v2).unwrap().clips();
        let on_a2 = project.sequence().track(a2).unwrap().clips();
        assert_eq!((on_v2.len(), on_a2.len()), (2, 2));
        assert_ne!(on_v2[0].link, on_v2[1].link);
        assert_eq!(on_v2[1].link, on_a2[1].link);
        let mut project = before.clone();
        first.apply(&mut project).unwrap();
        first.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn an_import_reverts_completely() {
        let empty = Project::new(fps30(), (1920, 1080));
        let mut project = empty.clone();
        let mut command = import(&project, "clip.mp4".into(), info(true, true), Frame(0));
        command.apply(&mut project).unwrap();
        command.revert(&mut project);
        assert_eq!(project, empty);
    }

    /// A project with a 2 s file with sound, MediaId(1), already on V1 and A1 from frame 0.
    fn occupied() -> Project {
        let mut project = Project::new(fps30(), (1920, 1080));
        import(&project, "first.mp4".into(), info(true, true), Frame(0))
            .apply(&mut project)
            .unwrap();
        project
    }

    fn add(project: &mut Project, info: MediaInfo) -> MediaId {
        let (id, mut command) = add_media(project, "next".into(), info);
        command.apply(project).unwrap();
        id
    }

    /// The tracks (by index) that hold clips of `media`.
    fn tracks_of(project: &Project, media: MediaId) -> Vec<usize> {
        let tracks = project.sequence().tracks().iter().enumerate();
        tracks
            .filter(|(_, track)| track.clips().iter().any(|clip| clip.media_id == media))
            .map(|(index, _)| index)
            .collect()
    }

    #[test]
    fn media_goes_on_the_first_pair_of_tracks_with_room() {
        let mut project = occupied();
        let media = add(&mut project, info(true, true));
        place_where_free(&project, media, Frame(30))
            .unwrap()
            .apply(&mut project)
            .unwrap();
        // V2 and A2: V1 and A1 are taken at frame 30.
        assert_eq!(tracks_of(&project, media), [1, 3]);
        let media = add(&mut project, info(true, true));
        place_where_free(&project, media, Frame(60))
            .unwrap()
            .apply(&mut project)
            .unwrap();
        assert_eq!(tracks_of(&project, media), [0, 2]);
    }

    #[test]
    fn sound_alone_takes_the_first_free_audio_track() {
        let mut project = occupied();
        let music = add(&mut project, info(false, true));
        place_where_free(&project, music, Frame(0))
            .unwrap()
            .apply(&mut project)
            .unwrap();
        assert_eq!(tracks_of(&project, music), [3]);
    }

    #[test]
    fn a_locked_pair_is_passed_over() {
        let mut project = Project::new(fps30(), (1920, 1080));
        let v1 = project.sequence().tracks()[0].id();
        Command::SetTrackLocked(crate::command::SetTrackLocked::new(v1, true))
            .apply(&mut project)
            .unwrap();
        let media = add(&mut project, info(true, true));
        place_where_free(&project, media, Frame(0))
            .unwrap()
            .apply(&mut project)
            .unwrap();
        assert_eq!(tracks_of(&project, media), [1, 3]);
    }

    #[test]
    fn with_no_room_anywhere_the_first_pair_says_why() {
        let mut project = occupied();
        let media = add(&mut project, info(true, true));
        place_where_free(&project, media, Frame(0))
            .unwrap()
            .apply(&mut project)
            .unwrap();
        let v1 = project.sequence().tracks()[0].id();
        let third = add(&mut project, info(true, true));
        assert_eq!(
            place_where_free(&project, third, Frame(10)).map(|_| ()),
            Err(Rejection::Overlap(v1))
        );
        assert_eq!(
            place_where_free(&project, MediaId(99), Frame(0)).map(|_| ()),
            Err(Rejection::UnknownMedia(MediaId(99)))
        );
    }

    #[test]
    fn a_placement_snaps_to_the_nearest_room() {
        let mut project = occupied(); // MediaId(1) on V1 and A1, frames 0..60
        let media = add(&mut project, info(true, true)); // 60 frames
        let ids: Vec<TrackId> = project.sequence().tracks().iter().map(|t| t.id()).collect();
        let (v1, v2, a1, a2) = (ids[0], ids[1], ids[2], ids[3]);
        assert_eq!(
            nearest_free_place(&project, media, Frame(30), v1, a1),
            Some(Frame(60))
        );
        assert_eq!(
            nearest_free_place(&project, media, Frame(90), v1, a1),
            Some(Frame(90))
        );
        assert_eq!(
            nearest_free_place(&project, media, Frame(10), v2, a2),
            Some(Frame(10))
        );
        assert_eq!(
            nearest_free_place(&project, media, Frame(-5), v2, a2),
            Some(Frame(0))
        );
        Command::SetTrackLocked(crate::command::SetTrackLocked::new(v2, true))
            .apply(&mut project)
            .unwrap();
        assert_eq!(nearest_free_place(&project, media, Frame(10), v2, a2), None);
        // Nothing of the project changed.
        assert_eq!(project.sequence().tracks()[0].clips().len(), 1);
    }

    #[test]
    fn media_dragged_onto_an_empty_timeline_starts_at_the_beginning() {
        // Wherever it is let go, so a first clip has no black before it.
        let mut project = Project::new(fps30(), (1920, 1080));
        let media = add(&mut project, info(true, true));
        let ids: Vec<TrackId> = project.sequence().tracks().iter().map(|t| t.id()).collect();
        let (v1, v2, a1, a2) = (ids[0], ids[1], ids[2], ids[3]);
        assert_eq!(
            drop_place(&project, media, Frame(240), v1, a1),
            Some(Frame(0))
        );
        assert_eq!(
            drop_place(&project, media, Frame(8), v2, a2),
            Some(Frame(0))
        );
        // A locked track still takes nothing.
        Command::SetTrackLocked(crate::command::SetTrackLocked::new(v1, true))
            .apply(&mut project)
            .unwrap();
        assert_eq!(drop_place(&project, media, Frame(240), v1, a1), None);
    }

    #[test]
    fn media_dragged_onto_a_timeline_with_clips_starts_where_it_is_let_go() {
        let mut project = occupied(); // MediaId(1) on V1 and A1, frames 0..60
        let media = add(&mut project, info(true, true));
        let ids: Vec<TrackId> = project.sequence().tracks().iter().map(|t| t.id()).collect();
        let (v1, v2, a1, a2) = (ids[0], ids[1], ids[2], ids[3]);
        assert_eq!(
            drop_place(&project, media, Frame(240), v1, a1),
            Some(Frame(240))
        );
        // Tracks of its own are not an empty timeline, and a taken place snaps as before.
        assert_eq!(
            drop_place(&project, media, Frame(240), v2, a2),
            Some(Frame(240))
        );
        assert_eq!(
            drop_place(&project, media, Frame(30), v1, a1),
            Some(Frame(60))
        );
    }
}
