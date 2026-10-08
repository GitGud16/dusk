//! Where a media file is: relinking one that moved or could not be found
//! (docs/ARCHITECTURE.md, "Release (0.1)").

use std::path::PathBuf;

use crate::command::{Rejection, check_clip};
use crate::model::{Clip, MediaId, MediaInfo, MediaRef, Project, TrackKind};

/// Points a media file of the project at another path, such as where a moved file went,
/// with what the file there holds. Its clips must still fit it: the same kind of media,
/// sound for the clips that play some, every clip's source range inside it and every crop
/// inside its picture. The project file then names the new path.
#[derive(Clone, Debug)]
pub struct RelinkMedia {
    media: MediaId,
    path: PathBuf,
    info: MediaInfo,
    /// The path and what it held before, once applied.
    before: Option<(PathBuf, MediaInfo)>,
}

impl RelinkMedia {
    /// Points `media` at `path`, whose file holds `info`.
    pub fn new(media: MediaId, path: PathBuf, info: MediaInfo) -> RelinkMedia {
        RelinkMedia {
            media,
            path,
            info,
            before: None,
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        let index = project
            .media
            .iter()
            .position(|media| media.id == self.media)
            .ok_or(Rejection::UnknownMedia(self.media))?;
        if project.media[index].info.kind != self.info.kind {
            return Err(Rejection::RelinkKind(self.media));
        }
        let clips: Vec<Clip> = project
            .clips()
            .filter(|(_, clip)| clip.media_id == self.media)
            .map(|(_, clip)| clip.clone())
            .collect();
        if !self.info.has_audio && clips.iter().any(|clip| clip.kind() == TrackKind::Audio) {
            return Err(Rejection::RelinkNoSound(self.media));
        }
        let relinked = MediaRef {
            id: self.media,
            path: self.path.clone(),
            info: self.info.clone(),
        };
        let before = std::mem::replace(&mut project.media[index], relinked);
        // The clips' own checks say whether they still fit: their source ranges inside the
        // file, their crops inside its picture.
        if let Some(rejection) = clips
            .iter()
            .find_map(|clip| check_clip(project, clip).err())
        {
            project.media[index] = before;
            return Err(match rejection {
                Rejection::SourceRange(_) => Rejection::RelinkTooShort(self.media),
                Rejection::Crop(_) => Rejection::RelinkCropSize(self.media),
                other => other,
            });
        }
        self.before = Some((before.path, before.info));
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        let Some((path, info)) = self.before.take() else {
            return;
        };
        if let Some(media) = project
            .media
            .iter_mut()
            .find(|media| media.id == self.media)
        {
            (media.path, media.info) = (path, info);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::*;
    use crate::command::{Command, SetVideoEdits};
    use crate::model::{ClipEdits, MediaKind, Rect};
    use crate::time::MediaTime;
    use std::path::Path;

    fn info(project: &Project) -> MediaInfo {
        project.media_ref(MediaId(1)).unwrap().info.clone()
    }

    fn relink(project: &mut Project, info: MediaInfo) -> Result<Command, Rejection> {
        let mut command = Command::RelinkMedia(RelinkMedia::new(
            MediaId(1),
            "D:/moved/clip.mp4".into(),
            info,
        ));
        command.apply(project).map(|()| command)
    }

    fn path(project: &Project) -> &Path {
        &project.media_ref(MediaId(1)).unwrap().path
    }

    #[test]
    fn a_relinked_file_is_used_from_its_new_place_until_undone() {
        let mut project = project();
        insert_pair(&mut project, 0, (0, 8 * SECOND));
        let mut longer = info(&project);
        longer.duration = MediaTime(12 * SECOND);
        let mut command = relink(&mut project, longer.clone()).unwrap();
        assert_eq!(path(&project), Path::new("D:/moved/clip.mp4"));
        assert_eq!(info(&project), longer);
        command.revert(&mut project);
        assert_eq!(path(&project), Path::new("clip.mp4"));
        assert_eq!(info(&project).duration, MediaTime(10 * SECOND));
    }

    #[test]
    fn another_kind_of_media_is_refused() {
        let mut project = project();
        insert_pair(&mut project, 0, (0, 8 * SECOND));
        let before = project.clone();
        let mut song = info(&project);
        song.kind = MediaKind::Audio;
        assert_eq!(
            relink(&mut project, song).err(),
            Some(Rejection::RelinkKind(MediaId(1)))
        );
        assert_eq!(project, before);
    }

    #[test]
    fn a_file_without_the_sound_a_clip_plays_is_refused() {
        let mut project = project();
        insert_pair(&mut project, 0, (0, 8 * SECOND));
        let mut silent = info(&project);
        silent.has_audio = false;
        assert_eq!(
            relink(&mut project, silent).err(),
            Some(Rejection::RelinkNoSound(MediaId(1)))
        );
        assert_eq!(path(&project), Path::new("clip.mp4"));
    }

    #[test]
    fn a_file_shorter_than_its_clips_is_refused() {
        let mut project = project();
        insert_pair(&mut project, 0, (0, 8 * SECOND));
        let mut shorter = info(&project);
        shorter.duration = MediaTime(5 * SECOND);
        assert_eq!(
            relink(&mut project, shorter).err(),
            Some(Rejection::RelinkTooShort(MediaId(1)))
        );
        assert_eq!(path(&project), Path::new("clip.mp4"));
        // Long enough for every clip: fine, though shorter than before.
        let mut enough = info(&project);
        enough.duration = MediaTime(8 * SECOND);
        assert!(relink(&mut project, enough).is_ok());
    }

    #[test]
    fn a_smaller_picture_is_refused_only_where_a_crop_reaches_outside_it() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 0, (0, 8 * SECOND));
        let mut smaller = info(&project);
        (smaller.width, smaller.height) = (1280, 720);
        // Without a crop, any picture size fits.
        let mut command = relink(&mut project, smaller.clone()).unwrap();
        command.revert(&mut project);
        let ClipEdits::Video(mut edits) = project.find_clip(video).unwrap().1.edits.clone() else {
            panic!("a video clip");
        };
        edits.crop = Some(Rect {
            x: 100,
            y: 100,
            width: 1600,
            height: 900,
        });
        Command::SetVideoEdits(SetVideoEdits::new(video, edits))
            .apply(&mut project)
            .unwrap();
        assert_eq!(
            relink(&mut project, smaller).err(),
            Some(Rejection::RelinkCropSize(MediaId(1)))
        );
        assert_eq!(path(&project), Path::new("clip.mp4"));
    }

    #[test]
    fn media_the_project_does_not_have_is_refused() {
        let mut project = project();
        let mut command = Command::RelinkMedia(RelinkMedia::new(
            MediaId(9),
            "elsewhere.mp4".into(),
            info(&project),
        ));
        assert_eq!(
            command.apply(&mut project),
            Err(Rejection::UnknownMedia(MediaId(9)))
        );
    }
}
