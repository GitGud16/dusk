//! Setting a clip's picture edits (fit, crop, rotation, flips) or sound edits (volume,
//! fades). Each applies to one clip: a link partner has edits of the other kind.

use crate::command::{Rejection, check_audio_edits, check_video_edits, clip_indices};
use crate::model::{AudioEdits, ClipEdits, ClipId, Project, VideoEdits};

/// Sets the picture edits of a video clip.
#[derive(Clone, Debug)]
pub struct SetVideoEdits {
    clip: ClipId,
    edits: VideoEdits,
    before: Option<VideoEdits>,
}

impl SetVideoEdits {
    /// Gives video clip `clip` the edits `edits`.
    pub fn new(clip: ClipId, edits: VideoEdits) -> SetVideoEdits {
        SetVideoEdits {
            clip,
            edits,
            before: None,
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        let (t, i) = clip_indices(project, self.clip)?;
        let track = &project.sequence.tracks[t];
        if track.locked {
            return Err(Rejection::TrackLocked(track.id));
        }
        let clip = &track.clips[i];
        let ClipEdits::Video(current) = &clip.edits else {
            return Err(Rejection::NotVideo(self.clip));
        };
        check_video_edits(project, clip, &self.edits)?;
        self.before = Some(current.clone());
        if let Some(edits) = edits_mut(project, self.clip) {
            *edits = ClipEdits::Video(self.edits.clone());
        }
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        if let (Some(before), Some(edits)) = (self.before.take(), edits_mut(project, self.clip)) {
            *edits = ClipEdits::Video(before);
        }
    }
}

/// Sets the sound edits of an audio clip.
#[derive(Clone, Debug)]
pub struct SetAudioEdits {
    clip: ClipId,
    edits: AudioEdits,
    before: Option<AudioEdits>,
}

impl SetAudioEdits {
    /// Gives audio clip `clip` the edits `edits`.
    pub fn new(clip: ClipId, edits: AudioEdits) -> SetAudioEdits {
        SetAudioEdits {
            clip,
            edits,
            before: None,
        }
    }

    pub(super) fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        let (t, i) = clip_indices(project, self.clip)?;
        let track = &project.sequence.tracks[t];
        if track.locked {
            return Err(Rejection::TrackLocked(track.id));
        }
        let clip = &track.clips[i];
        let ClipEdits::Audio(current) = &clip.edits else {
            return Err(Rejection::NotAudio(self.clip));
        };
        check_audio_edits(clip, &self.edits)?;
        self.before = Some(current.clone());
        if let Some(edits) = edits_mut(project, self.clip) {
            *edits = ClipEdits::Audio(self.edits.clone());
        }
        Ok(())
    }

    pub(super) fn revert(&mut self, project: &mut Project) {
        if let (Some(before), Some(edits)) = (self.before.take(), edits_mut(project, self.clip)) {
            *edits = ClipEdits::Audio(before);
        }
    }
}

/// The edits of clip `id`, wherever it is.
fn edits_mut(project: &mut Project, id: ClipId) -> Option<&mut ClipEdits> {
    let (t, i) = clip_indices(project, id).ok()?;
    Some(&mut project.sequence.tracks[t].clips[i].edits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::*;
    use crate::command::{Command, Edge};
    use crate::model::{Fit, Rect, TrackKind};
    use crate::time::Frame;

    fn audio_edits(volume_db: f32, fade_in: i64, fade_out: i64) -> AudioEdits {
        AudioEdits {
            volume_db,
            fade_in: Frame(fade_in),
            fade_out: Frame(fade_out),
        }
    }

    fn audio_of(project: &Project, id: ClipId) -> AudioEdits {
        match clip(project, id).edits {
            ClipEdits::Audio(edits) => edits,
            ClipEdits::Video(_) => panic!("not an audio clip"),
        }
    }

    fn video_of(project: &Project, id: ClipId) -> VideoEdits {
        match clip(project, id).edits {
            ClipEdits::Video(edits) => edits,
            ClipEdits::Audio(_) => panic!("not a video clip"),
        }
    }

    #[test]
    fn volume_and_fades_are_set_and_reverted() {
        let mut project = project();
        let (_, audio) = insert_pair(&mut project, 0, (0, 2 * SECOND)); // 60 frames
        let before = project.clone();
        let edits = audio_edits(-6.0, 15, 30);
        let mut set = Command::SetAudioEdits(SetAudioEdits::new(audio, edits.clone()));
        set.apply(&mut project).unwrap();
        assert_eq!(audio_of(&project, audio), edits);
        set.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn sound_edits_must_fit_the_clip() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 2 * SECOND)); // 60 frames
        let refused = |edits| {
            let mut project = project.clone();
            Command::SetAudioEdits(SetAudioEdits::new(audio, edits)).apply(&mut project)
        };
        assert_eq!(
            refused(audio_edits(0.0, 40, 30)),
            Err(Rejection::Fades(audio))
        );
        assert_eq!(
            refused(audio_edits(0.0, -1, 0)),
            Err(Rejection::Fades(audio))
        );
        assert_eq!(
            refused(audio_edits(20.0, 0, 0)),
            Err(Rejection::Volume(audio))
        );
        assert_eq!(
            refused(audio_edits(f32::NAN, 0, 0)),
            Err(Rejection::Volume(audio))
        );
        let mut project = project.clone();
        assert_eq!(
            Command::SetAudioEdits(SetAudioEdits::new(video, AudioEdits::default()))
                .apply(&mut project),
            Err(Rejection::NotAudio(video))
        );
        assert_eq!(
            Command::SetVideoEdits(SetVideoEdits::new(audio, VideoEdits::default()))
                .apply(&mut project),
            Err(Rejection::NotVideo(audio))
        );
    }

    #[test]
    fn fill_is_set_and_reverted() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 0, (0, SECOND));
        let before = project.clone();
        let fill = VideoEdits {
            fit: Fit::Fill,
            ..VideoEdits::default()
        };
        let mut set = Command::SetVideoEdits(SetVideoEdits::new(video, fill.clone()));
        set.apply(&mut project).unwrap();
        assert_eq!(video_of(&project, video), fill);
        set.revert(&mut project);
        assert_eq!(project, before);
    }

    #[test]
    fn a_crop_must_lie_inside_the_picture() {
        let mut project = project(); // 1920x1080
        let (video, _) = insert_pair(&mut project, 0, (0, SECOND));
        let crop = |x, y, width, height| VideoEdits {
            crop: Some(Rect {
                x,
                y,
                width,
                height,
            }),
            ..VideoEdits::default()
        };
        let mut inside = project.clone();
        Command::SetVideoEdits(SetVideoEdits::new(video, crop(100, 100, 1280, 720)))
            .apply(&mut inside)
            .unwrap();
        for outside in [
            crop(1000, 0, 1000, 500),
            crop(0, 600, 100, 500),
            crop(0, 0, 0, 10),
        ] {
            assert_eq!(
                Command::SetVideoEdits(SetVideoEdits::new(video, outside))
                    .apply(&mut project.clone()),
                Err(Rejection::Crop(video))
            );
        }
    }

    #[test]
    fn edits_on_a_locked_track_are_refused() {
        let mut project = project();
        let (_, audio) = insert_pair(&mut project, 0, (0, SECOND));
        let audio_track = track(&project, TrackKind::Audio);
        lock(&mut project, audio_track);
        assert_eq!(
            Command::SetAudioEdits(SetAudioEdits::new(audio, AudioEdits::default()))
                .apply(&mut project),
            Err(Rejection::TrackLocked(audio_track))
        );
    }

    #[test]
    fn a_trim_shorter_than_the_fades_shortens_them_and_reverts() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 4 * SECOND)); // 120 frames
        Command::SetAudioEdits(SetAudioEdits::new(audio, audio_edits(0.0, 40, 60)))
            .apply(&mut project)
            .unwrap();
        let before = project.clone();
        let mut command = trim(&mut project, video, Edge::End, 70).unwrap();
        let edits = audio_of(&project, audio);
        assert_eq!((edits.fade_in, edits.fade_out), (Frame(40), Frame(30)));
        command.revert(&mut project);
        assert_eq!(project, before);
    }
}
