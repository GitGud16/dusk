//! The pop-out clip editor's session (docs/ARCHITECTURE.md, "Pop-out clip editor"): one link
//! group's edits drafted apart from the timeline, previewed on their own, then applied to the
//! project in one step or exported as a file of their own.

use crate::command::{ApplyClipSession, Command, Rejection, check_clip};
use crate::model::{
    AudioEdits, Clip, ClipEdits, ClipId, Fit, MediaKind, Project, Rotation, VideoEdits,
};
use crate::time::{Frame, MediaTime, Rational, frame_to_media, length_for, media_to_frame};

/// What the clip editor changes in a link group, alike for every clip of it.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipDraft {
    /// Where the clips start in their source; 0 for a still.
    pub source_in: MediaTime,
    /// Where they end in their source; 0 for a still.
    pub source_out: MediaTime,
    /// How fast the source plays: 1 is as recorded.
    pub speed: f64,
    /// How long a still lasts on the timeline. Video and sound last as long as their source
    /// range at their speed.
    pub still_length: Frame,
    /// The picture edits, when the group has a video clip.
    pub video: Option<VideoEdits>,
    /// The sound edits, when the group has an audio clip.
    pub audio: Option<AudioEdits>,
}

impl ClipDraft {
    /// The draft of `clips`, a link group in lock-step.
    fn of(clips: &[Clip]) -> ClipDraft {
        let first = &clips[0];
        ClipDraft {
            source_in: first.source_in,
            source_out: first.source_out,
            speed: first.speed,
            still_length: first.length,
            video: clips.iter().find_map(|clip| match &clip.edits {
                ClipEdits::Video(edits) => Some(edits.clone()),
                ClipEdits::Audio(_) => None,
            }),
            audio: clips.iter().find_map(|clip| match &clip.edits {
                ClipEdits::Audio(edits) => Some(edits.clone()),
                ClipEdits::Video(_) => None,
            }),
        }
    }

    /// How long the clips last on a timeline at `rate`: a still as long as it is set to,
    /// video and sound as long as their source range at their speed.
    pub fn length(&self, still: bool, rate: Rational) -> Frame {
        if still {
            self.still_length
        } else {
            length_for(self.source_out - self.source_in, self.speed, rate)
        }
    }

    /// `clip` with the draft applied and lasting `length`, where it was.
    pub(crate) fn applied_to(&self, clip: &Clip, still: bool, length: Frame) -> Clip {
        let mut clip = clip.clone();
        if still {
            (clip.source_in, clip.source_out, clip.speed) = (MediaTime(0), MediaTime(0), 1.0);
        } else {
            (clip.source_in, clip.source_out, clip.speed) =
                (self.source_in, self.source_out, self.speed);
        }
        clip.length = length;
        match &mut clip.edits {
            ClipEdits::Video(edits) => {
                if let Some(video) = &self.video {
                    *edits = video.clone();
                }
            }
            ClipEdits::Audio(edits) => {
                if let Some(audio) = &self.audio {
                    *edits = audio.clone();
                }
            }
        }
        clip
    }
}

/// How a link group stands against what a session last saw of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    /// As it was.
    Current,
    /// Trimmed, moved, sped or edited in the main window: the draft can be reloaded from the
    /// group, or kept and applied over it.
    Edited,
    /// A clip of it was deleted or unlinked: the draft can no longer be applied, only
    /// exported.
    Broken,
}

/// The clip editor's work on one link group.
#[derive(Clone, Debug)]
pub struct ClipEditSession {
    /// The group's clips as the session last saw them.
    seen: Vec<Clip>,
    /// The edits being drafted.
    pub draft: ClipDraft,
}

impl ClipEditSession {
    /// Opens `clip` and every clip linked to it, the draft starting as they are.
    pub fn open(project: &Project, clip: ClipId) -> Result<ClipEditSession, Rejection> {
        let seen = group_clips(project, clip).ok_or(Rejection::UnknownClip(clip))?;
        Ok(ClipEditSession {
            draft: ClipDraft::of(&seen),
            seen,
        })
    }

    /// Whether the draft differs from the group as the session last saw it.
    pub fn changed(&self) -> bool {
        self.draft != ClipDraft::of(&self.seen)
    }

    /// The group's clips as the session last saw them, in timeline order.
    pub fn clips(&self) -> &[Clip] {
        &self.seen
    }

    /// The clips the session edits.
    pub fn group(&self) -> Vec<ClipId> {
        self.seen.iter().map(|clip| clip.id).collect()
    }

    /// How the group stands in `project`, against what the session last saw.
    pub fn status(&self, project: &Project) -> SessionStatus {
        match group_clips(project, self.seen[0].id) {
            Some(clips) if self.same_group(&clips) => {
                if clips == self.seen {
                    SessionStatus::Current
                } else {
                    SessionStatus::Edited
                }
            }
            _ => SessionStatus::Broken,
        }
    }

    /// Whether `clips` are the clips the session edits, in the same order.
    fn same_group(&self, clips: &[Clip]) -> bool {
        clips.len() == self.seen.len()
            && clips
                .iter()
                .zip(&self.seen)
                .all(|(clip, seen)| clip.id == seen.id)
    }

    /// Takes the draft from the group as it is in `project` now (Reload from project).
    pub fn reload(&mut self, project: &Project) -> Result<(), Rejection> {
        *self = ClipEditSession::open(project, self.seen[0].id)?;
        Ok(())
    }

    /// Takes the group as it is in `project` now as seen, keeping the draft, which applying
    /// then lays over it (Keep my draft). A broken group stays broken.
    pub fn keep_draft(&mut self, project: &Project) {
        if let Some(clips) = group_clips(project, self.seen[0].id)
            && self.same_group(&clips)
        {
            self.seen = clips;
        }
    }

    /// The command that applies the draft to the group (Apply to project).
    pub fn apply_command(&self) -> Command {
        Command::ApplyClipSession(ApplyClipSession::new(self.group(), self.draft.clone()))
    }

    /// A project of the group alone with the draft applied, from frame 0, at the sequence's
    /// rate and size: what the clip editor previews. A draft that breaks a rule of the
    /// timeline is refused, with the reason.
    pub fn preview_project(&self, project: &Project) -> Result<Project, Rejection> {
        let sequence = &project.sequence;
        self.alone(
            &self.draft,
            project,
            sequence.frame_rate,
            sequence.resolution,
        )
    }

    /// A project of the group alone with the draft applied, at the clip's own snapped rate
    /// (a still at the sequence's) and its upright size after the crop and the turn: what
    /// Export as file writes. The picture fills that frame, which has its shape, so an
    /// encoder that rounds the size down crops a pixel rather than leaving a bar.
    pub fn export_project(&self, project: &Project) -> Result<Project, Rejection> {
        let first = &self.seen[0];
        let media = project
            .media_ref(first.media_id)
            .ok_or(Rejection::UnknownMedia(first.media_id))?;
        let sequence = &project.sequence;
        let video = self.draft.video.as_ref().filter(|_| media.info.has_video);
        let rate = match media.info.kind {
            MediaKind::Video => media.info.frame_rate.unwrap_or(sequence.frame_rate),
            MediaKind::Still | MediaKind::Audio => sequence.frame_rate,
        };
        let size = video.map_or(sequence.resolution, |edits| {
            let (width, height) = edits
                .crop
                .map_or((media.info.width, media.info.height), |crop| {
                    (crop.width, crop.height)
                });
            match edits.rotate {
                Rotation::Quarter | Rotation::ThreeQuarters => (height, width),
                Rotation::None | Rotation::Half => (width, height),
            }
        });
        let mut draft = self.draft.clone();
        if let Some(edits) = draft.video.as_mut() {
            edits.fit = Fit::Fill;
        }
        self.alone(&draft, project, rate, size)
    }

    /// A project of the group alone with `draft` applied, from frame 0, at `rate` and
    /// `size`. Fades and a still's length, in frames of the sequence, keep their duration.
    fn alone(
        &self,
        draft: &ClipDraft,
        project: &Project,
        rate: Rational,
        size: (u32, u32),
    ) -> Result<Project, Rejection> {
        let first = &self.seen[0];
        let media = project
            .media_ref(first.media_id)
            .ok_or(Rejection::UnknownMedia(first.media_id))?;
        let still = media.info.kind == MediaKind::Still;
        let from = project.sequence.frame_rate;
        let duration = |frames: Frame| media_to_frame(frame_to_media(frames, from), rate);
        let mut draft = draft.clone();
        draft.still_length = duration(draft.still_length);
        if let Some(audio) = draft.audio.as_mut() {
            audio.fade_in = duration(audio.fade_in);
            audio.fade_out = duration(audio.fade_out);
        }
        let mut alone = Project::new(rate, size);
        alone.media.push(media.clone());
        let length = draft.length(still, rate);
        for seen in &self.seen {
            let mut clip = draft.applied_to(seen, still, length);
            clip.position = Frame(0);
            clip.enabled = true;
            check_clip(&alone, &clip)?;
            if let Some(track) = alone
                .sequence
                .tracks
                .iter_mut()
                .find(|track| track.kind == clip.kind())
            {
                track.clips.push(clip);
            }
        }
        Ok(alone)
    }
}

/// `clip` and every clip linked to it as they are in `project`, in timeline order; `None`
/// when `clip` is not there.
fn group_clips(project: &Project, clip: ClipId) -> Option<Vec<Clip>> {
    let group = project.link_group(clip);
    let clips: Vec<Clip> = project
        .clips()
        .filter(|(_, clip)| group.contains(&clip.id))
        .map(|(_, clip)| clip.clone())
        .collect();
    (!clips.is_empty()).then_some(clips)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::*;
    use crate::command::{Edge, Unlink, remove_one};
    use crate::model::{Clip, Fit, MediaId, Rect, Rotation, TrackKind};

    fn group_of(session: &ClipEditSession) -> Vec<ClipId> {
        let mut group = session.group();
        group.sort();
        group
    }

    #[test]
    fn opening_a_clip_drafts_its_whole_link_group() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 30, (SECOND, 5 * SECOND));
        let session = ClipEditSession::open(&project, audio).unwrap();
        let mut expected = vec![video, audio];
        expected.sort();
        assert_eq!(group_of(&session), expected);
        let draft = &session.draft;
        assert_eq!(
            (draft.source_in, draft.source_out, draft.speed),
            (MediaTime(SECOND), MediaTime(5 * SECOND), 1.0)
        );
        assert_eq!(draft.video, Some(VideoEdits::default()));
        assert_eq!(draft.audio, Some(AudioEdits::default()));
        assert_eq!(draft.length(false, fps30()), Frame(120));
    }

    #[test]
    fn a_clip_that_is_not_there_cannot_be_opened() {
        let project = project();
        assert_eq!(
            ClipEditSession::open(&project, ClipId(7)).map(|_| ()),
            Err(Rejection::UnknownClip(ClipId(7)))
        );
    }

    #[test]
    fn a_session_knows_whether_its_draft_differs_from_the_clips() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 0, (0, SECOND));
        let mut session = ClipEditSession::open(&project, video).unwrap();
        assert!(!session.changed());
        session.draft.speed = 2.0;
        assert!(session.changed());
        session.draft.speed = 1.0;
        assert!(!session.changed());
        session.draft.audio.as_mut().unwrap().volume_db = -6.0;
        assert!(session.changed());
    }

    #[test]
    fn the_preview_holds_the_group_alone_from_the_first_frame() {
        let mut project = project();
        insert_pair(&mut project, 0, (0, SECOND));
        let (video, _) = insert_pair(&mut project, 300, (SECOND, 5 * SECOND));
        let mut session = ClipEditSession::open(&project, video).unwrap();
        session.draft.source_out = MediaTime(3 * SECOND);
        let preview = session.preview_project(&project).unwrap();
        assert_eq!(preview.sequence().frame_rate(), fps30());
        assert_eq!(preview.sequence().resolution(), (1920, 1080));
        assert_eq!(preview.media().len(), 1);
        let clips: Vec<&Clip> = preview.clips().map(|(_, clip)| clip).collect();
        assert_eq!(clips.len(), 2);
        for clip in clips {
            assert_eq!((clip.position, clip.length), (Frame(0), Frame(60)));
            assert!(clip.enabled);
        }
    }

    #[test]
    fn a_draft_that_breaks_a_rule_has_no_preview() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 0, (0, 5 * SECOND));
        let mut session = ClipEditSession::open(&project, video).unwrap();
        session.draft.speed = 40.0;
        assert!(matches!(
            session.preview_project(&project),
            Err(Rejection::Speed(_))
        ));
    }

    #[test]
    fn edits_in_the_main_window_show_and_the_draft_is_kept_or_reloaded() {
        let mut project = project();
        let (video, _) = insert_pair(&mut project, 0, (0, 5 * SECOND));
        let mut session = ClipEditSession::open(&project, video).unwrap();
        session.draft.speed = 2.0;
        assert_eq!(session.status(&project), SessionStatus::Current);
        trim(&mut project, video, Edge::End, 100).unwrap();
        assert_eq!(session.status(&project), SessionStatus::Edited);

        let mut kept = session.clone();
        kept.keep_draft(&project);
        assert_eq!(kept.status(&project), SessionStatus::Current);
        assert_eq!(kept.draft.speed, 2.0);

        session.reload(&project).unwrap();
        assert_eq!(session.status(&project), SessionStatus::Current);
        assert_eq!(
            (session.draft.speed, session.draft.source_out),
            (1.0, MediaTime(3_333_333))
        );
    }

    #[test]
    fn deleting_or_unlinking_a_clip_of_the_group_breaks_the_session() {
        let mut project = project();
        let (video, audio) = insert_pair(&mut project, 0, (0, 5 * SECOND));
        let mut session = ClipEditSession::open(&project, video).unwrap();
        let mut unlinked = project.clone();
        Command::Unlink(Unlink::new(video))
            .apply(&mut unlinked)
            .unwrap();
        assert_eq!(session.status(&unlinked), SessionStatus::Broken);
        let mut deleted = project.clone();
        remove_one(&deleted, audio).apply(&mut deleted).unwrap();
        assert_eq!(session.status(&deleted), SessionStatus::Broken);
        // Keeping the draft does not mend it.
        session.keep_draft(&deleted);
        assert_eq!(session.status(&deleted), SessionStatus::Broken);
    }

    /// A 25 fps 1280x720 sequence holding the fixture's 30 fps 1920x1080 clip with sound.
    fn unlike_sequence() -> (Project, ClipId) {
        let source = project();
        let media = source.media()[0].clone();
        let mut project = Project::new(Rational::new(25, 1).unwrap(), (1280, 720));
        Command::AddMedia(media).apply(&mut project).unwrap();
        let mut command = crate::import::place(
            &project,
            MediaId(1),
            Frame(0),
            track(&project, TrackKind::Video),
            track(&project, TrackKind::Audio),
        )
        .unwrap();
        command.apply(&mut project).unwrap();
        let video = project.sequence().tracks()[0].clips()[0].id;
        (project, video)
    }

    #[test]
    fn exporting_uses_the_clips_own_rate_and_its_cropped_turned_size() {
        let (project, video) = unlike_sequence();
        let mut session = ClipEditSession::open(&project, video).unwrap();
        session.draft.source_out = MediaTime(2 * SECOND);
        let edits = session.draft.video.as_mut().unwrap();
        edits.crop = Some(Rect {
            x: 100,
            y: 100,
            width: 640,
            height: 360,
        });
        edits.rotate = Rotation::Quarter;
        let export = session.export_project(&project).unwrap();
        assert_eq!(export.sequence().frame_rate(), fps30());
        assert_eq!(export.sequence().resolution(), (360, 640));
        let clips: Vec<&Clip> = export.clips().map(|(_, clip)| clip).collect();
        assert_eq!(clips.len(), 2);
        for clip in clips {
            // 2 s at the clip's own 30 fps.
            assert_eq!((clip.position, clip.length), (Frame(0), Frame(60)));
            // The frame has the picture's own shape: an encoder that rounds its size down
            // must not leave a hairline bar.
            if let ClipEdits::Video(edits) = &clip.edits {
                assert_eq!(edits.fit, Fit::Fill);
            }
        }
        // The draft itself keeps fitting with bars, as the timeline shows it.
        assert_eq!(session.draft.video.unwrap().fit, Fit::Fit);
    }

    #[test]
    fn a_still_previews_and_exports_for_as_long_as_it_is_drafted() {
        let mut project = project();
        let photo = add_still(&mut project);
        let still = insert_still(&mut project, photo, 0, 150);
        let mut session = ClipEditSession::open(&project, still).unwrap();
        assert_eq!(session.draft.still_length, Frame(150));
        session.draft.still_length = Frame(90);
        let preview = session.preview_project(&project).unwrap();
        assert_eq!(preview.sequence().end(), Frame(90));
        let export = session.export_project(&project).unwrap();
        assert_eq!(export.sequence().frame_rate(), fps30());
        assert_eq!(export.sequence().resolution(), (4032, 3024));
        assert_eq!(export.sequence().end(), Frame(90));
    }
}
