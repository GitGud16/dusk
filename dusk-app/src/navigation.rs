//! Where the keys that edit without the mouse go (docs/ARCHITECTURE.md, "Keyboard and
//! settings"): the cuts before and after the playhead, a second away, the clips at the
//! playhead from the top of the timeline down, and the media before or after the selected.

use dusk_core::{ClipId, Frame, MediaId, Project, Rational};

use crate::timeline::{nominal_fps, track_rows};

/// Every cut, in order: the sequence's start, and each clip's start and end on any track.
fn cuts(project: &Project) -> Vec<Frame> {
    let ends = project
        .sequence()
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .flat_map(|clip| [clip.position, clip.end()]);
    let mut cuts: Vec<Frame> = std::iter::once(Frame(0)).chain(ends).collect();
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

/// The nearest cut after `frame`: the start or end of a clip on any track.
pub fn cut_after(project: &Project, frame: Frame) -> Option<Frame> {
    cuts(project).into_iter().find(|cut| *cut > frame)
}

/// The nearest cut before `frame`, the sequence's start counting as one.
pub fn cut_before(project: &Project, frame: Frame) -> Option<Frame> {
    cuts(project).into_iter().rev().find(|cut| *cut < frame)
}

/// A second at `rate`, in whole frames, as timecodes count it: 30 at 29.97 fps.
pub fn one_second(rate: Rational) -> i64 {
    nominal_fps(rate)
}

/// The clips at `frame`, from the top of the timeline down: V2's, V1's, then A1's and A2's.
pub fn clips_at(project: &Project, frame: Frame) -> Vec<ClipId> {
    let tracks = project.sequence().tracks();
    track_rows(project)
        .iter()
        .filter_map(|row| tracks.iter().find(|track| track.id() == row.id))
        .filter_map(|track| {
            track
                .clips()
                .iter()
                .find(|clip| clip.position <= frame && frame < clip.end())
        })
        .map(|clip| clip.id)
        .collect()
}

/// The clip to select at `frame`: the one below `selected` among the clips there, the
/// topmost again after the last, or the topmost when `selected` is not one of them.
pub fn next_clip_at(project: &Project, frame: Frame, selected: Option<ClipId>) -> Option<ClipId> {
    let clips = clips_at(project, frame);
    match selected.and_then(|selected| clips.iter().position(|clip| *clip == selected)) {
        Some(index) => clips.get((index + 1) % clips.len()).copied(),
        None => clips.first().copied(),
    }
}

/// The media before or after `selected` in the bin (`forward`), staying put at either end;
/// the first when none is selected.
pub fn neighbor_media(
    project: &Project,
    selected: Option<MediaId>,
    forward: bool,
) -> Option<MediaId> {
    let media: Vec<MediaId> = project.media().iter().map(|media| media.id).collect();
    let index = match selected.and_then(|selected| media.iter().position(|id| *id == selected)) {
        Some(index) if forward => (index + 1).min(media.len() - 1),
        Some(index) => index.saturating_sub(1),
        None => 0,
    };
    media.get(index).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{MediaInfo, MediaKind, MediaTime, TrackKind, add_media, import, place};

    fn rate(num: u32, den: u32) -> Rational {
        Rational::new(num, den).unwrap()
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
            frame_rate: has_video.then(|| rate(30, 1)),
            vfr: false,
            width: if has_video { 1920 } else { 0 },
            height: if has_video { 1080 } else { 0 },
            orientation: dusk_core::Orientation::UPRIGHT,
        }
    }

    /// A 30 fps project: a 2 s clip with sound at 15 on V1 and A1, a picture alone at 40 on
    /// V2, and sound alone at 80 on A2; and the clips' ids, top down.
    fn project() -> (Project, [ClipId; 4]) {
        let mut project = Project::new(rate(30, 1), (1920, 1080));
        import(&project, "a.mp4".into(), info(true, true), Frame(15))
            .apply(&mut project)
            .unwrap();
        let track = |project: &Project, kind: TrackKind, nth: usize| {
            project
                .sequence()
                .tracks()
                .iter()
                .filter(|track| track.kind() == kind)
                .nth(nth)
                .unwrap()
                .id()
        };
        let (v1, v2) = (
            track(&project, TrackKind::Video, 0),
            track(&project, TrackKind::Video, 1),
        );
        let (a1, a2) = (
            track(&project, TrackKind::Audio, 0),
            track(&project, TrackKind::Audio, 1),
        );
        for (has_video, at) in [(true, 40), (false, 80)] {
            let (media, mut add) = add_media(&project, "b".into(), info(has_video, !has_video));
            add.apply(&mut project).unwrap();
            place(&project, media, Frame(at), v2, a2)
                .unwrap()
                .apply(&mut project)
                .unwrap();
        }
        let first_on = |project: &Project, track| {
            project
                .sequence()
                .tracks()
                .iter()
                .find(|each| each.id() == track)
                .unwrap()
                .clips()[0]
                .id
        };
        let clips = [
            first_on(&project, v2),
            first_on(&project, v1),
            first_on(&project, a1),
            first_on(&project, a2),
        ];
        (project, clips)
    }

    #[test]
    fn up_and_down_go_from_cut_to_cut() {
        let (project, _) = project();
        // Cuts at 15, 40, 75, 80, 100 and 140, and the start.
        let mut after = Vec::new();
        let mut at = Frame(0);
        while let Some(next) = cut_after(&project, at) {
            after.push(next.0);
            at = next;
        }
        assert_eq!(after, [15, 40, 75, 80, 100, 140]);
        assert_eq!(cut_after(&project, Frame(50)), Some(Frame(75)));
        assert_eq!(cut_before(&project, Frame(50)), Some(Frame(40)));
        assert_eq!(cut_before(&project, Frame(40)), Some(Frame(15)));
        assert_eq!(cut_before(&project, Frame(15)), Some(Frame(0)));
        assert_eq!(cut_before(&project, Frame(0)), None);
        assert_eq!(cut_after(&project, Frame(140)), None);
        let empty = Project::new(rate(30, 1), (1920, 1080));
        assert_eq!(cut_after(&empty, Frame(0)), None);
    }

    #[test]
    fn a_second_is_whole_frames() {
        assert_eq!(one_second(rate(30, 1)), 30);
        assert_eq!(one_second(rate(30000, 1001)), 30);
        assert_eq!(one_second(rate(24000, 1001)), 24);
        assert_eq!(one_second(rate(25, 1)), 25);
    }

    #[test]
    fn the_clips_at_a_frame_come_from_the_top_down() {
        let (project, [v2, v1, a1, a2]) = project();
        assert_eq!(clips_at(&project, Frame(50)), [v2, v1, a1]);
        assert_eq!(clips_at(&project, Frame(90)), [v2, a2]);
        assert_eq!(clips_at(&project, Frame(15)), [v1, a1]);
        // A clip ends before the frame it ends at.
        assert_eq!(clips_at(&project, Frame(140)), []);
    }

    #[test]
    fn selecting_at_the_playhead_goes_down_and_around() {
        let (project, [v2, v1, a1, a2]) = project();
        let at = Frame(50);
        assert_eq!(next_clip_at(&project, at, None), Some(v2));
        assert_eq!(next_clip_at(&project, at, Some(v2)), Some(v1));
        assert_eq!(next_clip_at(&project, at, Some(v1)), Some(a1));
        assert_eq!(next_clip_at(&project, at, Some(a1)), Some(v2));
        // A clip elsewhere starts from the top.
        assert_eq!(next_clip_at(&project, at, Some(a2)), Some(v2));
        assert_eq!(next_clip_at(&project, Frame(141), Some(v2)), None);
    }

    #[test]
    fn the_media_before_and_after_stay_put_at_the_ends() {
        let (project, _) = project();
        let media: Vec<MediaId> = project.media().iter().map(|media| media.id).collect();
        assert_eq!(media.len(), 3);
        assert_eq!(neighbor_media(&project, None, true), Some(media[0]));
        assert_eq!(neighbor_media(&project, None, false), Some(media[0]));
        assert_eq!(
            neighbor_media(&project, Some(media[0]), true),
            Some(media[1])
        );
        assert_eq!(
            neighbor_media(&project, Some(media[2]), true),
            Some(media[2])
        );
        assert_eq!(
            neighbor_media(&project, Some(media[1]), false),
            Some(media[0])
        );
        assert_eq!(
            neighbor_media(&project, Some(media[0]), false),
            Some(media[0])
        );
        let empty = Project::new(rate(30, 1), (1920, 1080));
        assert_eq!(neighbor_media(&empty, None, true), None);
    }
}
