//! What the timeline shows, worked out from the project: timecodes, the ruler and the clips.

use dusk_core::{Frame, Project, Rational, TrackKind};

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

/// A labeled mark on the ruler.
#[derive(Debug, PartialEq)]
pub struct Tick {
    pub frame: Frame,
    pub label: String,
}

/// Ticks across `width` pixels at `zoom` pixels per frame, on round timecodes and at least
/// [`TICK_GAP`] pixels apart.
pub fn ticks(width: f32, zoom: f32, rate: Rational) -> Vec<Tick> {
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
    let last = (width / zoom.max(f32::MIN_POSITIVE)) as i64;
    (0..=last / step)
        .map(|n| {
            let frame = Frame(n * step);
            Tick {
                frame,
                label: timecode(frame, rate),
            }
        })
        .collect()
}

/// The smallest gap between ruler ticks, in pixels: room for a timecode label.
pub const TICK_GAP: f32 = 96.0;

/// A clip as the timeline draws it.
#[derive(Debug, PartialEq)]
pub struct ClipRow {
    pub id: u64,
    /// The row: the track's index, video tracks first.
    pub track: usize,
    pub start: Frame,
    pub length: Frame,
    /// The media file's name.
    pub name: String,
    pub link: Option<u64>,
    pub video: bool,
}

/// Every clip of `project`, by track.
pub fn clip_rows(project: &Project) -> Vec<ClipRow> {
    let tracks = project.sequence().tracks().iter().enumerate();
    tracks
        .flat_map(|(index, track)| {
            track.clips().iter().map(move |clip| {
                let name = project
                    .media_ref(clip.media_id)
                    .and_then(|media| media.path.file_name())
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                ClipRow {
                    id: clip.id.0,
                    track: index,
                    start: clip.position,
                    length: clip.length,
                    name,
                    link: clip.link.map(|link| link.0),
                    video: track.kind() == TrackKind::Video,
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{MediaInfo, MediaKind, MediaTime, import};

    fn rate(num: u32, den: u32) -> Rational {
        Rational::new(num, den).unwrap()
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
        let close = ticks(250.0, 10.0, fps30);
        let frames: Vec<i64> = close.iter().map(|tick| tick.frame.0).collect();
        assert_eq!(frames, [0, 10, 20]);
        assert_eq!(close[2].label, "00:00:00:20");
        // 1 px a frame: 5 seconds are 150 px.
        let far = ticks(400.0, 1.0, fps30);
        let frames: Vec<i64> = far.iter().map(|tick| tick.frame.0).collect();
        assert_eq!(frames, [0, 150, 300]);
        assert_eq!(far[1].label, "00:00:05:00");
    }

    #[test]
    fn a_linked_import_gives_two_rows() {
        let mut project = Project::new(rate(30, 1), (1920, 1080));
        let info = MediaInfo {
            kind: MediaKind::Video,
            duration: MediaTime(2_000_000),
            has_video: true,
            has_audio: true,
            frame_rate: Some(rate(30, 1)),
            vfr: false,
            width: 1920,
            height: 1080,
        };
        import(&project, "C:/clips/beach.mp4".into(), info, Frame(15))
            .apply(&mut project)
            .unwrap();
        let rows = clip_rows(&project);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].track, rows[0].video), (0, true));
        assert_eq!((rows[1].track, rows[1].video), (1, false));
        for row in &rows {
            assert_eq!((row.start, row.length), (Frame(15), Frame(60)));
            assert_eq!(row.name, "beach.mp4");
            assert!(row.link.is_some());
        }
        assert_eq!(rows[0].link, rows[1].link);
    }
}
