//! The clip editor's changes to its draft (docs/ARCHITECTURE.md, "Pop-out clip editor") and
//! what its window shows of it. The crop is shown as the pixels cut from each side of the
//! picture as it looks after the turn and the flips; the model keeps it as a rectangle of the
//! upright source.

use dusk_core::time::{frame_to_media, media_to_frame, source_span};
use dusk_core::{
    ClipDraft, ClipEditSession, ClipEdits, Edge, Fit, Frame, MediaKind, MediaTime, Project,
    Rational, Rect, Rotation, VideoEdits,
};

use crate::timeline::{media_name, timecode, track_rows};

/// What a draft is cut from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    /// How long the source lasts; 0 for a still.
    pub duration: MediaTime,
    /// The rate trim points snap to: the source's own frame rate, or the sequence's for
    /// sound.
    pub grid: Rational,
    /// The upright picture size; 0×0 without video.
    pub size: (u32, u32),
    pub still: bool,
}

impl Source {
    /// The source of `session`'s clips in `project`; `None` when its media is gone.
    pub fn of(project: &Project, session: &ClipEditSession) -> Option<Source> {
        let clip = session.clips().first()?;
        let info = &project.media_ref(clip.media_id)?.info;
        Some(Source {
            duration: info.duration,
            grid: info
                .frame_rate
                .unwrap_or_else(|| project.sequence().frame_rate()),
            size: (info.width, info.height),
            still: info.kind == MediaKind::Still,
        })
    }
}

/// Pixels cut from each side of the picture as the clip shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sides {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl Sides {
    /// The same cut seen after a quarter turn clockwise: the left edge goes to the top, the
    /// top to the right, the right to the bottom and the bottom to the left.
    fn turned(self) -> Sides {
        Sides {
            left: self.bottom,
            top: self.left,
            right: self.top,
            bottom: self.right,
        }
    }

    /// The same cut seen after `turns` quarter turns clockwise.
    fn turned_by(self, turns: usize) -> Sides {
        (0..turns % 4).fold(self, |sides, _| sides.turned())
    }

    /// The same cut seen mirrored left to right (`horizontal`) and top to bottom
    /// (`vertical`).
    fn flipped(self, horizontal: bool, vertical: bool) -> Sides {
        let (left, right) = if horizontal {
            (self.right, self.left)
        } else {
            (self.left, self.right)
        };
        let (top, bottom) = if vertical {
            (self.bottom, self.top)
        } else {
            (self.top, self.bottom)
        };
        Sides {
            left,
            top,
            right,
            bottom,
        }
    }
}

/// Quarter turns clockwise, in order.
const TURNS: [Rotation; 4] = [
    Rotation::None,
    Rotation::Quarter,
    Rotation::Half,
    Rotation::ThreeQuarters,
];

fn quarter_turns(rotation: Rotation) -> usize {
    TURNS.iter().position(|turn| *turn == rotation).unwrap_or(0)
}

/// The sides `edits`' crop cuts from a picture of upright `size`, as the clip shows it.
pub fn shown_sides(edits: &VideoEdits, size: (u32, u32)) -> Sides {
    let upright = edits.crop.map_or_else(Sides::default, |crop| Sides {
        left: crop.x,
        top: crop.y,
        right: size.0.saturating_sub(crop.x.saturating_add(crop.width)),
        bottom: size.1.saturating_sub(crop.y.saturating_add(crop.height)),
    });
    upright
        .turned_by(quarter_turns(edits.rotate))
        .flipped(edits.flip_h, edits.flip_v)
}

/// Crops `edits` so that it cuts `sides` from a picture of upright `size`, as the clip
/// shows it. Sides that leave nothing make a crop the timeline refuses.
pub fn set_shown_sides(edits: &mut VideoEdits, size: (u32, u32), sides: Sides) {
    let upright = sides
        .flipped(edits.flip_h, edits.flip_v)
        .turned_by(4 - quarter_turns(edits.rotate));
    edits.crop = (upright != Sides::default()).then(|| Rect {
        x: upright.left,
        y: upright.top,
        width: size
            .0
            .saturating_sub(upright.left.saturating_add(upright.right)),
        height: size
            .1
            .saturating_sub(upright.top.saturating_add(upright.bottom)),
    });
}

/// Turns the picture a quarter clockwise or counterclockwise as the clip shows it. A
/// mirrored picture stays mirrored the same way on screen, and the crop turns with it.
pub fn turn(edits: &mut VideoEdits, clockwise: bool) {
    let turns = quarter_turns(edits.rotate) + if clockwise { 1 } else { 3 };
    edits.rotate = TURNS[turns % 4];
    // The flips apply after the turn: a quarter turn of what shows swaps them.
    std::mem::swap(&mut edits.flip_h, &mut edits.flip_v);
}

/// Moves the draft's start or end in its source to `at`, on the source's frame grid, kept
/// inside the source and at least a frame from the other end.
pub fn trim(draft: &mut ClipDraft, edge: Edge, at: MediaTime, source: &Source) {
    let at = frame_to_media(media_to_frame(at, source.grid), source.grid);
    let frame = frame_to_media(Frame(1), source.grid);
    match edge {
        Edge::Start => {
            draft.source_in = at.min(draft.source_out - frame).max(MediaTime(0));
        }
        Edge::End => {
            draft.source_out = at.max(draft.source_in + frame).min(source.duration);
        }
    }
}

/// The source time shown `playhead` frames into the drafted clip, at the sequence's `rate`.
pub fn source_time_at(draft: &ClipDraft, playhead: Frame, rate: Rational) -> MediaTime {
    draft.source_in + source_span(playhead, draft.speed, rate)
}

/// The frame of the drafted clip, `length` frames long at `rate`, that shows the source at
/// `at`; the first or the last frame for a time outside the clip.
pub fn playhead_at(draft: &ClipDraft, at: MediaTime, rate: Rational, length: Frame) -> Frame {
    let into = MediaTime(((at - draft.source_in).0 as f64 / draft.speed).round() as i64);
    media_to_frame(into, rate).clamp(Frame(0), (length - Frame(1)).max(Frame(0)))
}

/// Starts the draft at the frame under the playhead (`Edge::Start`), or ends it after that
/// frame (`Edge::End`); `playhead` counts frames into the drafted clip at `rate`.
pub fn mark(draft: &mut ClipDraft, edge: Edge, playhead: Frame, rate: Rational, source: &Source) {
    let frames = match edge {
        Edge::Start => playhead,
        Edge::End => playhead + Frame(1),
    };
    let at = source_time_at(draft, frames, rate);
    trim(draft, edge, at, source);
}

/// Shortens the fades to fit the draft's length at `rate`, as a trim on the timeline does.
pub fn fit_fades(draft: &mut ClipDraft, source: &Source, rate: Rational) {
    let length = draft.length(source.still, rate);
    if let Some(audio) = draft.audio.as_mut() {
        audio.fit_into(length);
    }
}

/// What the clip editor window shows of a session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorView {
    pub name: String,
    /// What the clips are and where, such as "Video and audio on V1 and A1".
    pub place: String,
    pub video: bool,
    pub audio: bool,
    pub still: bool,
    /// Where the clips start and end in their source, as fractions of it, for the trim bar.
    pub in_point: f32,
    pub out_point: f32,
    /// The same as timecodes at the source's rate.
    pub source_in: String,
    pub source_out: String,
    /// How long the clips last, at the sequence's rate.
    pub length: String,
    pub speed_percent: i32,
    /// A still's length in frames of the sequence.
    pub frames: i64,
    /// Turned clockwise by this many degrees: 0, 90, 180 or 270.
    pub turn_degrees: i32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub fill: bool,
    pub crop: Sides,
    /// The picture's size after the crop and the turn, such as "1080 × 1920".
    pub picture: String,
    pub volume: f32,
    pub fade_in: i64,
    pub fade_out: i64,
    /// A clip of the group sits on a locked track, so the draft cannot be applied.
    pub locked: bool,
    /// The draft differs from the clips.
    pub changed: bool,
}

/// What the clip editor shows of `session`, whose clips are in `project`.
pub fn editor_view(project: &Project, session: &ClipEditSession) -> EditorView {
    let Some(source) = Source::of(project, session) else {
        return EditorView::default();
    };
    let clips = session.clips();
    let draft = &session.draft;
    let rate = project.sequence().frame_rate();
    let video = clips
        .iter()
        .any(|clip| matches!(clip.edits, ClipEdits::Video(_)));
    let audio = clips
        .iter()
        .any(|clip| matches!(clip.edits, ClipEdits::Audio(_)));
    let what = match (video, audio) {
        (true, true) => "Video and audio",
        (true, false) if source.still => "Photo",
        (true, false) => "Video",
        (false, _) => "Audio",
    };
    // Where the clips are now; a deleted one is nowhere.
    let rows = track_rows(project);
    let placed: Vec<(String, bool)> = clips
        .iter()
        .filter_map(|clip| project.find_clip(clip.id))
        .filter_map(|(track, _)| {
            let row = rows.iter().find(|row| row.id == track.id())?;
            Some((row.name.clone(), row.locked))
        })
        .collect();
    let names: Vec<&str> = placed.iter().map(|(name, _)| name.as_str()).collect();
    let place = match names.as_slice() {
        [] => what.to_owned(),
        names => format!("{what} on {}", names.join(" and ")),
    };
    let fraction = |time: MediaTime| match source.duration.0 {
        0 => 0.0,
        duration => (time.0 as f64 / duration as f64) as f32,
    };
    let source_timecode =
        |time: MediaTime| timecode(media_to_frame(time, source.grid), source.grid);
    let mut view = EditorView {
        name: media_name(project, clips[0].media_id),
        place,
        video,
        audio,
        still: source.still,
        in_point: if source.still {
            0.0
        } else {
            fraction(draft.source_in)
        },
        out_point: if source.still {
            1.0
        } else {
            fraction(draft.source_out)
        },
        source_in: source_timecode(draft.source_in),
        source_out: source_timecode(draft.source_out),
        length: timecode(draft.length(source.still, rate), rate),
        speed_percent: (draft.speed * 100.0).round() as i32,
        frames: draft.still_length.0,
        locked: placed.iter().any(|(_, locked)| *locked),
        changed: session.changed(),
        ..EditorView::default()
    };
    if let Some(edits) = &draft.video {
        let cropped = edits
            .crop
            .map_or(source.size, |crop| (crop.width, crop.height));
        let (width, height) = match edits.rotate {
            Rotation::Quarter | Rotation::ThreeQuarters => (cropped.1, cropped.0),
            Rotation::None | Rotation::Half => cropped,
        };
        view.turn_degrees = quarter_turns(edits.rotate) as i32 * 90;
        view.flip_h = edits.flip_h;
        view.flip_v = edits.flip_v;
        view.fill = edits.fit == Fit::Fill;
        view.crop = shown_sides(edits, source.size);
        view.picture = format!("{width} × {height}");
    }
    if let Some(edits) = &draft.audio {
        view.volume = edits.volume_db;
        view.fade_in = edits.fade_in.0;
        view.fade_out = edits.fade_out.0;
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{AudioEdits, Command, MediaInfo, Orientation, add_media, import, place};

    const SECOND: i64 = 1_000_000;

    fn rate(num: u32, den: u32) -> Rational {
        Rational::new(num, den).unwrap()
    }

    fn cut(left: u32, top: u32, right: u32, bottom: u32) -> Sides {
        Sides {
            left,
            top,
            right,
            bottom,
        }
    }

    /// Edits that cut `upright` sides from a 100×50 picture, turned and flipped as given.
    fn edits(upright: Sides, rotate: Rotation, flip_h: bool, flip_v: bool) -> VideoEdits {
        let mut edits = VideoEdits::default();
        set_shown_sides(&mut edits, (100, 50), upright);
        VideoEdits {
            rotate,
            flip_h,
            flip_v,
            ..edits
        }
    }

    const ROTATIONS: [Rotation; 4] = [
        Rotation::None,
        Rotation::Quarter,
        Rotation::Half,
        Rotation::ThreeQuarters,
    ];

    #[test]
    fn the_cut_sides_turn_with_the_picture() {
        let upright = cut(10, 5, 0, 0);
        let shown = |rotate| shown_sides(&edits(upright, rotate, false, false), (100, 50));
        assert_eq!(shown(Rotation::None), cut(10, 5, 0, 0));
        // A quarter turn clockwise takes the left edge to the top, the top to the right.
        assert_eq!(shown(Rotation::Quarter), cut(0, 10, 5, 0));
        assert_eq!(shown(Rotation::Half), cut(0, 0, 10, 5));
        assert_eq!(shown(Rotation::ThreeQuarters), cut(5, 0, 0, 10));
        let mirrored = edits(upright, Rotation::None, true, false);
        assert_eq!(shown_sides(&mirrored, (100, 50)), cut(0, 5, 10, 0));
    }

    #[test]
    fn sides_set_as_shown_come_back_as_shown() {
        let sides = cut(1, 2, 3, 4);
        for rotate in ROTATIONS {
            for (flip_h, flip_v) in [(false, false), (true, false), (false, true), (true, true)] {
                let mut edits = edits(Sides::default(), rotate, flip_h, flip_v);
                set_shown_sides(&mut edits, (100, 50), sides);
                assert_eq!(shown_sides(&edits, (100, 50)), sides, "{rotate:?}");
            }
        }
    }

    #[test]
    fn the_crop_is_kept_in_upright_pixels() {
        // Turned a quarter clockwise, the left of what shows is the bottom of the source.
        let mut edits = edits(Sides::default(), Rotation::Quarter, false, false);
        set_shown_sides(&mut edits, (100, 50), cut(20, 0, 0, 0));
        assert_eq!(
            edits.crop,
            Some(Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 30
            })
        );
        set_shown_sides(&mut edits, (100, 50), Sides::default());
        assert_eq!(edits.crop, None);
    }

    #[test]
    fn turning_a_mirrored_picture_turns_what_shows() {
        // Cut on the left of the source and mirrored, the cut shows on the right; turned
        // clockwise, the right side goes to the bottom.
        let before = edits(cut(10, 0, 0, 0), Rotation::None, true, false);
        let mut turned = before.clone();
        turn(&mut turned, true);
        assert_eq!(shown_sides(&turned, (100, 50)), cut(0, 0, 0, 10));
        turn(&mut turned, false);
        assert_eq!(turned, before);
        for _ in 0..4 {
            turn(&mut turned, true);
        }
        assert_eq!(turned, before);
    }

    fn draft_of(source_in: i64, source_out: i64, speed: f64) -> ClipDraft {
        ClipDraft {
            source_in: MediaTime(source_in),
            source_out: MediaTime(source_out),
            speed,
            still_length: Frame(0),
            video: Some(VideoEdits::default()),
            audio: Some(AudioEdits {
                volume_db: 0.0,
                fade_in: Frame(30),
                fade_out: Frame(30),
            }),
        }
    }

    fn ten_seconds() -> Source {
        Source {
            duration: MediaTime(10 * SECOND),
            grid: rate(30, 1),
            size: (1920, 1080),
            still: false,
        }
    }

    #[test]
    fn trims_land_on_source_frames_inside_the_source() {
        let source = ten_seconds();
        let frame = 33_333;
        let mut draft = draft_of(SECOND, 5 * SECOND, 1.0);
        trim(&mut draft, Edge::Start, MediaTime(2_010_000), &source);
        assert_eq!(draft.source_in, MediaTime(2 * SECOND));
        trim(&mut draft, Edge::Start, MediaTime(6 * SECOND), &source);
        assert_eq!(draft.source_in, MediaTime(5 * SECOND - frame));
        trim(&mut draft, Edge::Start, MediaTime(-SECOND), &source);
        assert_eq!(draft.source_in, MediaTime(0));
        trim(&mut draft, Edge::End, MediaTime(20 * SECOND), &source);
        assert_eq!(draft.source_out, MediaTime(10 * SECOND));
        trim(&mut draft, Edge::End, MediaTime(0), &source);
        assert_eq!(draft.source_out, MediaTime(frame));
    }

    #[test]
    fn marks_take_the_frame_under_the_playhead() {
        let (source, fps30) = (ten_seconds(), rate(30, 1));
        let mut draft = draft_of(SECOND, 5 * SECOND, 1.0);
        assert_eq!(
            source_time_at(&draft, Frame(30), fps30),
            MediaTime(2 * SECOND)
        );
        mark(&mut draft, Edge::Start, Frame(30), fps30, &source);
        assert_eq!(draft.source_in, MediaTime(2 * SECOND));
        // The frame under the playhead is the last one kept.
        mark(&mut draft, Edge::End, Frame(29), fps30, &source);
        assert_eq!(draft.source_out, MediaTime(3 * SECOND));
        // At double speed a frame of the clip is two of the source.
        let mut fast = draft_of(SECOND, 5 * SECOND, 2.0);
        mark(&mut fast, Edge::Start, Frame(15), fps30, &source);
        assert_eq!(fast.source_in, MediaTime(2 * SECOND));
    }

    #[test]
    fn a_source_time_finds_the_frame_of_the_clip_that_shows_it() {
        let fps30 = rate(30, 1);
        // From 1 s at double speed, 4 s of source last 60 frames.
        let draft = draft_of(SECOND, 5 * SECOND, 2.0);
        let length = Frame(60);
        assert_eq!(
            playhead_at(&draft, MediaTime(3 * SECOND), fps30, length),
            Frame(30)
        );
        assert_eq!(
            playhead_at(&draft, MediaTime(SECOND / 2), fps30, length),
            Frame(0)
        );
        assert_eq!(
            playhead_at(&draft, MediaTime(9 * SECOND), fps30, length),
            Frame(59)
        );
    }

    #[test]
    fn fades_are_shortened_when_the_clip_gets_shorter() {
        let source = ten_seconds();
        let mut draft = draft_of(SECOND, 5 * SECOND, 1.0);
        fit_fades(&mut draft, &source, rate(30, 1));
        let fades = |draft: &ClipDraft| {
            let audio = draft.audio.as_ref().unwrap();
            (audio.fade_in, audio.fade_out)
        };
        assert_eq!(fades(&draft), (Frame(30), Frame(30)));
        trim(&mut draft, Edge::End, MediaTime(2_333_333), &source);
        fit_fades(&mut draft, &source, rate(30, 1));
        assert_eq!(fades(&draft), (Frame(30), Frame(10)));
    }

    fn info(kind: MediaKind) -> MediaInfo {
        MediaInfo {
            kind,
            duration: MediaTime(if kind == MediaKind::Still {
                0
            } else {
                10 * SECOND
            }),
            has_video: kind != MediaKind::Audio,
            has_audio: kind != MediaKind::Still,
            frame_rate: (kind == MediaKind::Video).then(|| rate(25, 1)),
            vfr: false,
            width: if kind == MediaKind::Audio { 0 } else { 1920 },
            height: if kind == MediaKind::Audio { 0 } else { 1080 },
            orientation: Orientation::UPRIGHT,
        }
    }

    /// A 30 fps project with a 10 s clip with sound at frame 15, and its session.
    fn opened() -> (Project, ClipEditSession) {
        let mut project = Project::new(rate(30, 1), (1920, 1080));
        import(
            &project,
            "C:/clips/beach.mp4".into(),
            info(MediaKind::Video),
            Frame(15),
        )
        .apply(&mut project)
        .unwrap();
        let clip = project.sequence().tracks()[0].clips()[0].id;
        let session = ClipEditSession::open(&project, clip).unwrap();
        (project, session)
    }

    #[test]
    fn the_source_snaps_to_its_own_frames() {
        let (project, session) = opened();
        let source = Source::of(&project, &session).unwrap();
        assert_eq!(source.duration, MediaTime(10 * SECOND));
        assert_eq!(source.grid, rate(25, 1));
        assert_eq!(source.size, (1920, 1080));
        assert!(!source.still);
    }

    #[test]
    fn the_view_shows_the_draft() {
        let (project, mut session) = opened();
        let view = editor_view(&project, &session);
        assert_eq!(view.name, "beach.mp4");
        assert_eq!(view.place, "Video and audio on V1 and A1");
        assert!(view.video && view.audio && !view.still && !view.changed && !view.locked);
        assert_eq!((view.in_point, view.out_point), (0.0, 1.0));
        assert_eq!(view.source_out, "00:00:10:00");
        assert_eq!(view.length, "00:00:10:00");
        assert_eq!(view.picture, "1920 × 1080");
        session.draft.speed = 2.0;
        session.draft.source_in = MediaTime(2 * SECOND);
        let video = session.draft.video.as_mut().unwrap();
        turn(video, true);
        set_shown_sides(video, (1920, 1080), cut(0, 0, 0, 120));
        let view = editor_view(&project, &session);
        assert!(view.changed);
        assert_eq!(view.speed_percent, 200);
        assert_eq!(view.in_point, 0.2);
        assert_eq!(view.source_in, "00:00:02:00");
        assert_eq!(view.length, "00:00:04:00");
        assert_eq!(view.turn_degrees, 90);
        assert_eq!(view.crop, cut(0, 0, 0, 120));
        assert_eq!(view.picture, "1080 × 1800");
    }

    #[test]
    fn a_photo_shows_its_length_in_frames() {
        let mut project = Project::new(rate(30, 1), (1920, 1080));
        let (media, mut add) =
            add_media(&project, "C:/photos/cat.jpg".into(), info(MediaKind::Still));
        add.apply(&mut project).unwrap();
        let tracks = project.sequence().tracks();
        let (video, audio) = (tracks[0].id(), tracks[2].id());
        let mut placed: Command = place(&project, media, Frame(0), video, audio).unwrap();
        placed.apply(&mut project).unwrap();
        let clip = project.sequence().tracks()[0].clips()[0].id;
        let session = ClipEditSession::open(&project, clip).unwrap();
        let view = editor_view(&project, &session);
        assert!(view.video && view.still && !view.audio);
        assert_eq!(view.place, "Photo on V1");
        assert_eq!(view.frames, 150);
        assert_eq!(view.length, "00:00:05:00");
    }
}
