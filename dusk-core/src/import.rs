//! Bringing a media file into the project (docs/FEATURES.md, "Import").

use std::path::PathBuf;

use crate::command::{Command, InsertClips};
use crate::model::{Clip, MediaInfo, MediaRef, Project, TrackKind};
use crate::time::{Frame, MediaTime};

/// The command that adds the file at `path` to `project` under a new id and places it at
/// `position`: a video clip on the first video track and an audio clip on the first audio
/// track, linked when the file has both, each showing the whole file. Applying it can still be
/// refused, for example when the clips would overlap others.
pub fn import(project: &Project, path: PathBuf, info: MediaInfo, position: Frame) -> Command {
    let mut ids = project.fresh_ids();
    let media = MediaRef {
        id: ids.media(),
        path,
        info,
    };
    let rate = project.sequence().frame_rate();
    let link = (media.info.has_video && media.info.has_audio).then(|| ids.link());
    let source = (MediaTime(0), media.info.duration);
    let mut placements = Vec::new();
    for (wanted, kind) in [
        (media.info.has_video, TrackKind::Video),
        (media.info.has_audio, TrackKind::Audio),
    ] {
        let track = project
            .sequence()
            .tracks()
            .iter()
            .find(|t| t.kind() == kind);
        if let (true, Some(track)) = (wanted, track) {
            let mut clip = Clip::new(ids.clip(), media.id, kind, source, position, rate);
            clip.link = link;
            placements.push((track.id(), clip));
        }
    }
    Command::Batch(vec![
        Command::AddMedia(media),
        Command::InsertClips(InsertClips::new(placements)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MediaId, MediaKind};
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
    fn an_import_reverts_completely() {
        let empty = Project::new(fps30(), (1920, 1080));
        let mut project = empty.clone();
        let mut command = import(&project, "clip.mp4".into(), info(true, true), Frame(0));
        command.apply(&mut project).unwrap();
        command.revert(&mut project);
        assert_eq!(project, empty);
    }
}
