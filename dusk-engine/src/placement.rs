//! How the visible clip's picture is placed in the frame, for preview and export alike.

use dusk_core::{ClipEdits, Frame, Orientation, Project};
use dusk_render::Placement;

/// How the picture shown at `frame` is placed: upright as its file says, then cropped,
/// turned, mirrored and fitted as its clip says, against the sequence's shape whatever size
/// the frame is drawn at. Plain where no clip is visible.
pub(crate) fn placement_at(project: &Project, frame: Frame) -> Placement {
    let Some(clip) = project.sequence().visible_video_at(frame) else {
        return Placement::default();
    };
    let shape = Some(project.sequence().resolution());
    let orientation = project
        .media_ref(clip.media_id)
        .map_or(Orientation::UPRIGHT, |media| media.info.orientation);
    match &clip.edits {
        ClipEdits::Video(edits) => Placement {
            orientation,
            crop: edits.crop,
            edits: Orientation::from_edits(edits.rotate, edits.flip_h, edits.flip_v),
            fit: edits.fit,
            shape,
        },
        ClipEdits::Audio(_) => Placement {
            orientation,
            shape,
            ..Placement::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{
        Clip, ClipId, Command, Fit, InsertClips, MediaId, MediaInfo, MediaKind, MediaRef,
        MediaTime, Rational, Rect, Rotation, SetVideoEdits, TrackKind, VideoEdits,
    };

    #[test]
    fn the_visible_clip_says_how_its_picture_is_placed() {
        let rate = Rational::new(30, 1).unwrap();
        let mut project = Project::new(rate, (1920, 1080));
        let phone = MediaRef {
            id: MediaId(1),
            path: "phone.mp4".into(),
            info: MediaInfo {
                kind: MediaKind::Video,
                duration: MediaTime(2_000_000),
                has_video: true,
                has_audio: false,
                frame_rate: Some(rate),
                vfr: false,
                width: 1080,
                height: 1920,
                orientation: Orientation::new(1, false),
            },
        };
        Command::AddMedia(phone).apply(&mut project).unwrap();
        let clip = Clip::new(
            ClipId(1),
            MediaId(1),
            TrackKind::Video,
            (MediaTime(0), MediaTime(1_000_000)),
            Frame(0),
            rate,
        );
        let v1 = project.sequence().tracks()[0].id();
        Command::InsertClips(InsertClips::new(vec![(v1, clip)]))
            .apply(&mut project)
            .unwrap();
        let crop = Rect {
            x: 0,
            y: 0,
            width: 1080,
            height: 1080,
        };
        let edits = VideoEdits {
            crop: Some(crop),
            rotate: Rotation::Half,
            flip_h: false,
            flip_v: false,
            fit: Fit::Fill,
        };
        Command::SetVideoEdits(SetVideoEdits::new(ClipId(1), edits))
            .apply(&mut project)
            .unwrap();
        let placement = placement_at(&project, Frame(10));
        assert_eq!(placement.orientation, Orientation::new(1, false));
        assert_eq!(placement.crop, Some(crop));
        assert_eq!(placement.edits, Orientation::new(2, false));
        assert_eq!(placement.fit, Fit::Fill);
        // Fitted or filled against the sequence's shape, whatever size it is drawn at.
        assert_eq!(placement.shape, Some((1920, 1080)));
        // In a gap nothing is placed.
        assert_eq!(placement_at(&project, Frame(40)), Placement::default());
    }
}
