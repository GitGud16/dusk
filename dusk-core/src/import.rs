//! Bringing a media file into the project (docs/FEATURES.md, "Import").

use std::path::PathBuf;

use crate::command::{Command, InsertClips, Rejection};
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
}
