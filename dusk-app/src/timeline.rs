//! What the timeline, the media bin and the properties panel show, worked out from the
//! project: rows, clips, timecodes, the ruler and the view's zoom and scroll.

use dusk_core::time::{length_for, media_to_frame};
use dusk_core::{
    ClipEdits, ClipId, Fit, Frame, MediaId, MediaKind, Project, Rational, STILL_LENGTH, TrackId,
    TrackKind,
};

/// `frame` as non-drop-frame timecode, HH:MM:SS:FF, counting the rate's nominal frames per
/// second (30 for 29.97).
pub fn timecode(frame: Frame, rate: Rational) -> String {
    let fps = nominal_fps(rate);
    let frames = frame.0.max(0);
    let seconds = frames / fps;
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        frames % fps
    )
}

/// Whole frames per timecode second: the rate rounded up, so 29.97 counts 30.
fn nominal_fps(rate: Rational) -> i64 {
    i64::from(rate.num().div_ceil(rate.den())).max(1)
}

/// Pixels per frame that fit the sequence, a little room after its end, into `width` pixels;
/// at least a second is always in view.
pub fn zoom(width: f32, end: Frame, rate: Rational) -> f32 {
    let shown = (end.0 as f32 * 1.05).max(nominal_fps(rate) as f32);
    width.max(1.0) / shown
}

/// The closest and the furthest the timeline zooms, in pixels per frame.
const ZOOM_RANGE: (f32, f32) = (0.0005, 32.0);

/// How much of the timeline is in view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    /// The whole sequence, whatever its length.
    Fit,
    /// A fixed number of pixels per frame.
    Fixed(f32),
}

/// The timeline's view: its width in pixels, zoom and scroll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub width: f32,
    pub zoom: Zoom,
    /// The frame at the left edge, while zoomed.
    pub scroll: Frame,
}

impl View {
    /// A view `width` pixels wide that fits the sequence.
    pub fn new(width: f32) -> View {
        View {
            width,
            zoom: Zoom::Fit,
            scroll: Frame(0),
        }
    }

    /// Pixels per frame, for a sequence ending at `end`.
    pub fn pixels_per_frame(&self, end: Frame, rate: Rational) -> f32 {
        match self.zoom {
            Zoom::Fit => zoom(self.width, end, rate),
            Zoom::Fixed(zoom) => zoom,
        }
    }

    /// The frame at the left edge.
    pub fn first_frame(&self) -> Frame {
        match self.zoom {
            Zoom::Fit => Frame(0),
            Zoom::Fixed(_) => self.scroll,
        }
    }

    /// Zooms by `factor`, keeping frame `around` where it is on screen.
    pub fn zoom_by(&mut self, factor: f32, around: Frame, end: Frame, rate: Rational) {
        let old = self.pixels_per_frame(end, rate);
        let new = (old * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        let first = self.first_frame().0 as f32;
        // `around` stays put: (around - first) × old = (around - new first) × new.
        let new_first = around.0 as f32 - (around.0 as f32 - first) * old / new;
        self.zoom = Zoom::Fixed(new);
        self.scroll = Frame(new_first.round().max(0.0) as i64);
    }

    /// Scrolls by `frames`, never before the start.
    pub fn scroll_by(&mut self, frames: i64, end: Frame, rate: Rational) {
        if self.zoom == Zoom::Fit {
            self.zoom = Zoom::Fixed(self.pixels_per_frame(end, rate));
        }
        self.scroll = Frame((self.scroll.0 + frames).max(0));
    }

    /// Fits the whole sequence again.
    pub fn fit(&mut self) {
        self.zoom = Zoom::Fit;
        self.scroll = Frame(0);
    }

    /// Scrolls so `frame` is in view, as a playhead running off the right edge needs.
    pub fn follow(&mut self, frame: Frame) {
        let Zoom::Fixed(zoom) = self.zoom else {
            return;
        };
        let visible = (self.width / zoom) as i64;
        if frame < self.scroll || frame.0 >= self.scroll.0 + visible {
            self.scroll = Frame((frame.0 - visible / 10).max(0));
        }
    }
}

/// A labeled mark on the ruler.
#[derive(Debug, PartialEq)]
pub struct Tick {
    pub frame: Frame,
    pub label: String,
}

/// Ticks across `width` pixels from frame `first` at `zoom` pixels per frame, on round
/// timecodes and at least [`TICK_GAP`] pixels apart.
pub fn ticks(width: f32, zoom: f32, first: Frame, rate: Rational) -> Vec<Tick> {
    let fps = nominal_fps(rate);
    // Steps of whole frames below a second, then of whole seconds.
    let frames = [1, 2, 5, 10].into_iter().filter(|step| *step < fps);
    let seconds = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600]
        .into_iter()
        .map(|step| step * fps);
    let Some(step) = frames
        .chain(seconds)
        .find(|step| *step as f32 * zoom >= TICK_GAP)
    else {
        return Vec::new();
    };
    let last = first.0 + (width / zoom.max(f32::MIN_POSITIVE)) as i64;
    (first.0.div_euclid(step)..=last / step)
        .map(|n| {
            let frame = Frame(n * step);
            Tick {
                frame,
                label: timecode(frame, rate),
            }
        })
        .filter(|tick| tick.frame >= first)
        .collect()
}

/// The smallest gap between ruler ticks, in pixels: room for a timecode label.
pub const TICK_GAP: f32 = 96.0;

/// A track as a row of the timeline.
#[derive(Debug, PartialEq)]
pub struct TrackRow {
    pub id: TrackId,
    /// V1, V2, A1, A2.
    pub name: String,
    pub video: bool,
    pub locked: bool,
    pub muted: bool,
}

/// The tracks as rows, top to bottom: the video tracks with the one drawn over the others
/// first (V2 above V1), then the audio tracks in order (A1, A2).
pub fn track_rows(project: &Project) -> Vec<TrackRow> {
    let tracks = project.sequence().tracks();
    let named = |kind, letter| {
        tracks
            .iter()
            .filter(move |track| track.kind() == kind)
            .enumerate()
            .map(move |(index, track)| TrackRow {
                id: track.id(),
                name: format!("{letter}{}", index + 1),
                video: kind == TrackKind::Video,
                locked: track.locked(),
                muted: track.muted(),
            })
    };
    let mut rows: Vec<TrackRow> = named(TrackKind::Video, 'V').collect();
    rows.reverse();
    rows.extend(named(TrackKind::Audio, 'A'));
    rows
}

/// The tracks media dropped on `row` goes on: the row's own track for streams of its kind,
/// and the track in the same place among the other kind for the rest (V1 with A1, V2 with A2;
/// the last one where there are fewer). Video track first.
pub fn tracks_for_row(project: &Project, row: usize) -> Option<(TrackId, TrackId)> {
    let of_kind = |kind| -> Vec<TrackId> {
        let tracks = project.sequence().tracks().iter();
        tracks
            .filter(|track| track.kind() == kind)
            .map(|track| track.id())
            .collect()
    };
    let (video, audio) = (of_kind(TrackKind::Video), of_kind(TrackKind::Audio));
    let pick = |tracks: &[TrackId], index: usize| tracks.get(index).or(tracks.last()).copied();
    if row < video.len() {
        let index = video.len() - 1 - row;
        Some((video[index], pick(&audio, index)?))
    } else {
        let index = row - video.len();
        Some((pick(&video, index)?, *audio.get(index)?))
    }
}

/// A clip as the timeline draws it.
#[derive(Debug, PartialEq)]
pub struct ClipRow {
    pub id: ClipId,
    /// Its row in [`track_rows`].
    pub row: usize,
    pub start: Frame,
    pub length: Frame,
    /// The media file's name.
    pub name: String,
    pub link: Option<u64>,
    pub video: bool,
    pub enabled: bool,
}

/// Every clip of `project`, row by row.
pub fn clip_rows(project: &Project) -> Vec<ClipRow> {
    let rows = track_rows(project);
    let mut clips = Vec::new();
    for (row, track) in rows.iter().enumerate() {
        let Some(track) = project.sequence().track(track.id) else {
            continue;
        };
        clips.extend(track.clips().iter().map(|clip| ClipRow {
            id: clip.id,
            row,
            start: clip.position,
            length: clip.length,
            name: media_name(project, clip.media_id),
            link: clip.link.map(|link| link.0),
            video: track.kind() == TrackKind::Video,
            enabled: clip.enabled,
        }));
    }
    clips
}

fn media_name(project: &Project, media: MediaId) -> String {
    project
        .media_ref(media)
        .and_then(|media| media.path.file_name())
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// A media file as the bin lists it.
#[derive(Debug, PartialEq)]
pub struct MediaRow {
    pub id: MediaId,
    pub name: String,
    /// What it is, its size and its duration.
    pub detail: String,
    /// How long it lasts once placed, at the sequence's rate.
    pub length: Frame,
    pub video: bool,
    pub audio: bool,
}

/// The project's media files, in the order they were imported.
pub fn media_rows(project: &Project) -> Vec<MediaRow> {
    let rate = project.sequence().frame_rate();
    project
        .media()
        .iter()
        .map(|media| {
            let info = &media.info;
            let size = format!("{}×{}", info.width, info.height);
            let length = match info.kind {
                MediaKind::Still => media_to_frame(STILL_LENGTH, rate),
                _ => length_for(info.duration, 1.0, rate),
            };
            let duration = timecode(length, rate);
            let detail = match info.kind {
                MediaKind::Still => format!("Photo · {size}"),
                MediaKind::Audio => format!("Audio · {duration}"),
                MediaKind::Video if info.has_audio => format!("Video · {size} · {duration}"),
                MediaKind::Video => format!("Video, no sound · {size} · {duration}"),
            };
            MediaRow {
                id: media.id,
                name: media_name(project, media.id),
                detail,
                length,
                video: info.has_video,
                audio: info.has_audio && info.kind != MediaKind::Still,
            }
        })
        .collect()
}

/// The selected clip as the properties panel shows it.
#[derive(Debug, Default, PartialEq)]
pub struct ClipDetails {
    pub shown: bool,
    pub name: String,
    /// What it is and where, such as "Audio on A2".
    pub place: String,
    pub video: bool,
    pub still: bool,
    pub enabled: bool,
    pub linked: bool,
    pub locked: bool,
    pub start: String,
    pub length: String,
    pub frames: i64,
    pub fill: bool,
    pub volume: f32,
    pub fade_in: i64,
    pub fade_out: i64,
}

/// What the properties panel shows for `clip`; nothing when it is `None` or gone.
pub fn clip_details(project: &Project, clip: Option<ClipId>) -> ClipDetails {
    let Some((track, clip)) = clip.and_then(|id| project.find_clip(id)) else {
        return ClipDetails::default();
    };
    let rate = project.sequence().frame_rate();
    let still = project
        .media_ref(clip.media_id)
        .is_some_and(|media| media.info.kind == MediaKind::Still);
    let what = match &clip.edits {
        ClipEdits::Video(_) if still => "Photo",
        ClipEdits::Video(_) => "Video",
        ClipEdits::Audio(_) => "Audio",
    };
    let rows = track_rows(project);
    let track_name = rows
        .iter()
        .find(|row| row.id == track.id())
        .map_or("", |row| row.name.as_str());
    let mut details = ClipDetails {
        shown: true,
        name: media_name(project, clip.media_id),
        place: format!("{what} on {track_name}"),
        video: matches!(clip.edits, ClipEdits::Video(_)),
        still,
        enabled: clip.enabled,
        linked: clip.link.is_some(),
        locked: track.locked(),
        start: timecode(clip.position, rate),
        length: timecode(clip.length, rate),
        frames: clip.length.0,
        ..ClipDetails::default()
    };
    match &clip.edits {
        ClipEdits::Video(edits) => details.fill = edits.fit == Fit::Fill,
        ClipEdits::Audio(edits) => {
            details.volume = edits.volume_db;
            details.fade_in = edits.fade_in.0;
            details.fade_out = edits.fade_out.0;
        }
    }
    details
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{Command, MediaInfo, MediaTime, add_media, import};

    fn rate(num: u32, den: u32) -> Rational {
        Rational::new(num, den).unwrap()
    }

    fn info(kind: MediaKind, has_audio: bool) -> MediaInfo {
        let video = kind != MediaKind::Audio;
        MediaInfo {
            kind,
            duration: MediaTime(if kind == MediaKind::Still {
                0
            } else {
                2_000_000
            }),
            has_video: video,
            has_audio,
            frame_rate: (kind == MediaKind::Video).then(|| rate(30, 1)),
            vfr: false,
            width: if video { 1920 } else { 0 },
            height: if video { 1080 } else { 0 },
            orientation: dusk_core::Orientation::UPRIGHT,
        }
    }

    /// A 30 fps project with a 2 s clip with sound imported at frame 15.
    fn project() -> Project {
        let mut project = Project::new(rate(30, 1), (1920, 1080));
        import(
            &project,
            "C:/clips/beach.mp4".into(),
            info(MediaKind::Video, true),
            Frame(15),
        )
        .apply(&mut project)
        .unwrap();
        project
    }

    #[test]
    fn timecodes_count_hours_minutes_seconds_and_frames() {
        let fps30 = rate(30, 1);
        assert_eq!(timecode(Frame(0), fps30), "00:00:00:00");
        assert_eq!(timecode(Frame(30 * 61 + 5), fps30), "00:01:01:05");
        assert_eq!(timecode(Frame(30 * 3600), fps30), "01:00:00:00");
        assert_eq!(timecode(Frame(25), rate(25, 1)), "00:00:01:00");
        assert_eq!(timecode(Frame(-3), fps30), "00:00:00:00");
    }

    #[test]
    fn ntsc_rates_count_their_nominal_frames() {
        let ntsc = rate(30000, 1001);
        assert_eq!(timecode(Frame(29), ntsc), "00:00:00:29");
        assert_eq!(timecode(Frame(30), ntsc), "00:00:01:00");
        assert_eq!(timecode(Frame(23), rate(24000, 1001)), "00:00:00:23");
    }

    #[test]
    fn the_sequence_fits_the_width_with_room_after_it() {
        assert!((zoom(1050.0, Frame(100), rate(30, 1)) - 10.0).abs() < 1e-4);
        // An empty sequence still shows a second.
        assert!((zoom(300.0, Frame(0), rate(30, 1)) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn ticks_sit_on_round_timecodes_far_enough_apart() {
        let fps30 = rate(30, 1);
        // 10 px a frame: 10 frames are 100 px, the first step wide enough.
        let close = ticks(250.0, 10.0, Frame(0), fps30);
        let frames: Vec<i64> = close.iter().map(|tick| tick.frame.0).collect();
        assert_eq!(frames, [0, 10, 20]);
        assert_eq!(close[2].label, "00:00:00:20");
        // 1 px a frame: 5 seconds are 150 px.
        let far = ticks(400.0, 1.0, Frame(0), fps30);
        let frames: Vec<i64> = far.iter().map(|tick| tick.frame.0).collect();
        assert_eq!(frames, [0, 150, 300]);
        assert_eq!(far[1].label, "00:00:05:00");
    }

    #[test]
    fn scrolled_ticks_start_at_the_left_edge() {
        let ticks = ticks(250.0, 10.0, Frame(15), rate(30, 1));
        let frames: Vec<i64> = ticks.iter().map(|tick| tick.frame.0).collect();
        assert_eq!(frames, [20, 30, 40]);
    }

    #[test]
    fn zooming_keeps_the_frame_under_the_pointer_in_place() {
        let fps30 = rate(30, 1);
        let mut view = View::new(1000.0);
        let end = Frame(1000);
        let before = view.pixels_per_frame(end, fps30);
        view.zoom_by(2.0, Frame(500), end, fps30);
        let after = view.pixels_per_frame(end, fps30);
        assert!((after - 2.0 * before).abs() < 1e-4);
        let x = |view: &View, zoom: f32| (500 - view.first_frame().0) as f32 * zoom;
        assert!((x(&view, after) - 500.0 * before).abs() <= after);
        // Zooming far out around a frame in view stops at frame 0.
        view.zoom_by(0.1, Frame(600), end, fps30);
        assert_eq!(view.first_frame(), Frame(0));
        view.fit();
        assert_eq!(view.zoom, Zoom::Fit);
    }

    #[test]
    fn the_view_never_scrolls_before_the_start_and_follows_the_playhead() {
        let fps30 = rate(30, 1);
        let mut view = View::new(100.0);
        view.scroll_by(-50, Frame(300), fps30);
        assert_eq!(view.first_frame(), Frame(0));
        view.zoom = Zoom::Fixed(1.0); // 100 frames in view
        view.follow(Frame(250));
        assert_eq!(view.first_frame(), Frame(240));
        view.follow(Frame(260));
        assert_eq!(view.first_frame(), Frame(240));
    }

    #[test]
    fn rows_put_the_top_video_track_first_then_audio_in_order() {
        let project = project();
        let names: Vec<String> = track_rows(&project)
            .into_iter()
            .map(|row| row.name)
            .collect();
        assert_eq!(names, ["V2", "V1", "A1", "A2"]);
        let rows = clip_rows(&project);
        assert_eq!(rows.len(), 2);
        // V1 is the second row, A1 the third.
        assert_eq!((rows[0].row, rows[0].video), (1, true));
        assert_eq!((rows[1].row, rows[1].video), (2, false));
        for row in &rows {
            assert_eq!((row.start, row.length), (Frame(15), Frame(60)));
            assert_eq!(row.name, "beach.mp4");
            assert!(row.link.is_some() && row.enabled);
        }
        assert_eq!(rows[0].link, rows[1].link);
    }

    #[test]
    fn a_drop_on_a_row_pairs_v1_with_a1_and_v2_with_a2() {
        let project = project();
        let ids: Vec<TrackId> = project.sequence().tracks().iter().map(|t| t.id()).collect();
        let (v1, v2, a1, a2) = (ids[0], ids[1], ids[2], ids[3]);
        assert_eq!(tracks_for_row(&project, 0), Some((v2, a2)));
        assert_eq!(tracks_for_row(&project, 1), Some((v1, a1)));
        assert_eq!(tracks_for_row(&project, 2), Some((v1, a1)));
        assert_eq!(tracks_for_row(&project, 3), Some((v2, a2)));
        assert_eq!(tracks_for_row(&project, 4), None);
    }

    #[test]
    fn the_bin_says_what_each_file_is() {
        let mut project = project();
        for (path, kind, sound) in [
            ("song.mp3", MediaKind::Audio, true),
            ("photo.jpg", MediaKind::Still, false),
        ] {
            let (_, mut add) = add_media(&project, path.into(), info(kind, sound));
            add.apply(&mut project).unwrap();
        }
        let rows = media_rows(&project);
        let details: Vec<&str> = rows.iter().map(|row| row.detail.as_str()).collect();
        assert_eq!(
            details,
            [
                "Video · 1920×1080 · 00:00:02:00",
                "Audio · 00:00:02:00",
                "Photo · 1920×1080"
            ]
        );
        assert_eq!(rows[2].length, Frame(150));
        assert!(rows[2].video && !rows[2].audio);
        assert!(!rows[1].video && rows[1].audio);
    }

    #[test]
    fn the_panel_shows_the_selected_clip() {
        let mut project = project();
        assert!(!clip_details(&project, None).shown);
        let audio = clip_rows(&project)[1].id;
        let edits = dusk_core::AudioEdits {
            volume_db: -6.0,
            fade_in: Frame(10),
            fade_out: Frame(5),
        };
        Command::SetAudioEdits(dusk_core::SetAudioEdits::new(audio, edits))
            .apply(&mut project)
            .unwrap();
        let details = clip_details(&project, Some(audio));
        assert!(details.shown && details.linked && details.enabled && !details.video);
        assert_eq!(details.place, "Audio on A1");
        assert_eq!(
            (details.start.as_str(), details.length.as_str()),
            ("00:00:00:15", "00:00:02:00")
        );
        assert_eq!(
            (details.volume, details.fade_in, details.fade_out),
            (-6.0, 10, 5)
        );
        assert!(!clip_details(&project, Some(ClipId(99))).shown);
    }
}
