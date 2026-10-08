//! The project as plain data: the media it uses, its sequence, the tracks and their clips.
//!
//! `Project`, `Sequence` and `Track` expose their contents read-only; only commands change
//! them (docs/ARCHITECTURE.md, "Commands").

use std::path::PathBuf;

use crate::time::{Frame, MediaTime, Rational, length_for, source_span};

/// Identifies a media file within a project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MediaId(pub u64);

/// Identifies a clip within a project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClipId(pub u64);

/// Identifies a track within a sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrackId(pub u64);

/// Identifies a group of linked clips, such as the video and audio of one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LinkId(pub u64);

/// What a media file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    /// Moving pictures, with or without sound.
    Video,
    /// Sound only (cover art does not count as video).
    Audio,
    /// A single picture.
    Still,
}

/// What Dusk knows about a media file, read once when it is imported.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaInfo {
    /// What the file holds.
    pub kind: MediaKind,
    /// The container duration.
    pub duration: MediaTime,
    /// Whether the file has a video stream that is not cover art.
    pub has_video: bool,
    /// Whether the file has an audio stream.
    pub has_audio: bool,
    /// The video frame rate, snapped to a standard rate when the file's rate varies; `None`
    /// without video.
    pub frame_rate: Option<Rational>,
    /// Whether the video's frame rate varies (phone recordings); its frames are then placed by
    /// timestamp, never by counting.
    pub vfr: bool,
    /// Picture width in pixels, after rotation; 0 without video.
    pub width: u32,
    /// Picture height in pixels, after rotation; 0 without video.
    pub height: u32,
}

/// A media file the project uses. Dusk never modifies or copies it.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaRef {
    /// The id clips refer to it by.
    pub id: MediaId,
    /// Where the file is.
    pub path: PathBuf,
    /// What it holds.
    pub info: MediaInfo,
}

/// Whether a track holds video clips or audio clips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    /// Video clips only.
    Video,
    /// Audio clips only.
    Audio,
}

/// The edits of a clip. The variant also says whether the clip is a video or an audio clip.
#[derive(Clone, Debug, PartialEq)]
pub enum ClipEdits {
    /// A video clip's edits.
    Video(VideoEdits),
    /// An audio clip's edits.
    Audio(AudioEdits),
}

/// The edits of a video clip. Crop, rotation, flips and fit arrive with the pop-out editor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoEdits {}

/// The edits of an audio clip. Volume and fades arrive in M2.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioEdits {}

/// A piece of a media file placed on a track.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    /// The clip's id.
    pub id: ClipId,
    /// The media file it shows or plays.
    pub media_id: MediaId,
    /// Where it starts in the source.
    pub source_in: MediaTime,
    /// Where it ends in the source.
    pub source_out: MediaTime,
    /// Where it starts on the timeline.
    pub position: Frame,
    /// How long it lasts on the timeline; it always follows from the source range and speed.
    pub length: Frame,
    /// Playback speed, from 0.1 to 32.
    pub speed: f64,
    /// Disabled clips are skipped in preview and export.
    pub enabled: bool,
    /// Its link group; linked clips share source range, position, length and speed.
    pub link: Option<LinkId>,
    /// Its edits.
    pub edits: ClipEdits,
}

impl Clip {
    /// An enabled, unlinked clip of `kind` without edits that shows `source_in..source_out`
    /// of a media file at normal speed from `position` on. Its length follows from the source
    /// range at `rate`.
    pub fn new(
        id: ClipId,
        media_id: MediaId,
        kind: TrackKind,
        source: (MediaTime, MediaTime),
        position: Frame,
        rate: Rational,
    ) -> Clip {
        let (source_in, source_out) = source;
        Clip {
            id,
            media_id,
            source_in,
            source_out,
            position,
            length: length_for(source_out - source_in, 1.0, rate),
            speed: 1.0,
            enabled: true,
            link: None,
            edits: match kind {
                TrackKind::Video => ClipEdits::Video(VideoEdits::default()),
                TrackKind::Audio => ClipEdits::Audio(AudioEdits::default()),
            },
        }
    }

    /// The kind of track the clip belongs on.
    pub fn kind(&self) -> TrackKind {
        match self.edits {
            ClipEdits::Video(_) => TrackKind::Video,
            ClipEdits::Audio(_) => TrackKind::Audio,
        }
    }

    /// The first frame after the clip.
    pub fn end(&self) -> Frame {
        self.position + self.length
    }

    /// The source time shown at timeline frame `at`, which must lie within the clip.
    pub fn source_time_at(&self, at: Frame, rate: Rational) -> MediaTime {
        self.source_in + source_span(at - self.position, self.speed, rate)
    }
}

/// A row of clips of one kind, sorted by position and never overlapping.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub(crate) id: TrackId,
    pub(crate) kind: TrackKind,
    pub(crate) locked: bool,
    pub(crate) muted: bool,
    pub(crate) clips: Vec<Clip>,
}

impl Track {
    /// The track's id.
    pub fn id(&self) -> TrackId {
        self.id
    }

    /// Which clips it holds.
    pub fn kind(&self) -> TrackKind {
        self.kind
    }

    /// Whether commands may change its clips.
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Whether it is left out of preview and export.
    pub fn muted(&self) -> bool {
        self.muted
    }

    /// Its clips, sorted by position.
    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }
}

/// The timeline: its frame rate, picture size and tracks.
#[derive(Clone, Debug, PartialEq)]
pub struct Sequence {
    pub(crate) frame_rate: Rational,
    pub(crate) resolution: (u32, u32),
    pub(crate) tracks: Vec<Track>,
}

impl Sequence {
    /// Frames per second of the timeline.
    pub fn frame_rate(&self) -> Rational {
        self.frame_rate
    }

    /// Width and height of the picture in pixels.
    pub fn resolution(&self) -> (u32, u32) {
        self.resolution
    }

    /// The tracks, video tracks first.
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// The track with `id`.
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|track| track.id == id)
    }

    /// The video clip shown at `frame`: the enabled clip there on the topmost unmuted video
    /// track (later video tracks draw over earlier ones), or `None` in a gap, which is black.
    pub fn visible_video_at(&self, frame: Frame) -> Option<&Clip> {
        self.tracks
            .iter()
            .rev()
            .filter(|track| track.kind == TrackKind::Video && !track.muted)
            .find_map(|track| {
                // Clips are sorted and never overlap: only the last one starting at or before
                // `frame` can cover it.
                let after = track.clips.partition_point(|clip| clip.position <= frame);
                let clip = &track.clips[after.checked_sub(1)?];
                (clip.enabled && frame < clip.end()).then_some(clip)
            })
    }

    /// The first frame after the last clip; 0 for an empty sequence.
    pub fn end(&self) -> Frame {
        self.tracks
            .iter()
            .filter_map(|track| track.clips.last().map(Clip::end))
            .max()
            .unwrap_or(Frame(0))
    }

    pub(crate) fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|track| track.id == id)
    }
}

/// A Dusk project: the media files it uses and its sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub(crate) media: Vec<MediaRef>,
    pub(crate) sequence: Sequence,
}

impl Project {
    /// An empty project with one video track and one audio track (the M1 layout).
    pub fn new(frame_rate: Rational, resolution: (u32, u32)) -> Project {
        let track = |id, kind| Track {
            id: TrackId(id),
            kind,
            locked: false,
            muted: false,
            clips: Vec::new(),
        };
        Project {
            media: Vec::new(),
            sequence: Sequence {
                frame_rate,
                resolution,
                tracks: vec![track(1, TrackKind::Video), track(2, TrackKind::Audio)],
            },
        }
    }

    /// The media files, each listed once.
    pub fn media(&self) -> &[MediaRef] {
        &self.media
    }

    /// The media file with `id`.
    pub fn media_ref(&self, id: MediaId) -> Option<&MediaRef> {
        self.media.iter().find(|media| media.id == id)
    }

    /// The timeline.
    pub fn sequence(&self) -> &Sequence {
        &self.sequence
    }

    /// The clip with `id` and the track it is on.
    pub fn find_clip(&self, id: ClipId) -> Option<(&Track, &Clip)> {
        self.clips().find(|(_, clip)| clip.id == id)
    }

    /// `id` and every clip linked to it; empty if there is no such clip.
    pub fn link_group(&self, id: ClipId) -> Vec<ClipId> {
        match self.find_clip(id) {
            None => Vec::new(),
            Some((_, clip)) => match clip.link {
                None => vec![id],
                Some(link) => self
                    .clips()
                    .filter(|(_, other)| other.link == Some(link))
                    .map(|(_, other)| other.id)
                    .collect(),
            },
        }
    }

    /// Ids that nothing in the project uses yet, for new media, clips and links.
    pub fn fresh_ids(&self) -> FreshIds {
        let next = |ids: &mut dyn Iterator<Item = u64>| ids.max().map_or(1, |max| max + 1);
        FreshIds {
            media: next(&mut self.media.iter().map(|media| media.id.0)),
            clip: next(&mut self.clips().map(|(_, clip)| clip.id.0)),
            link: next(&mut self.clips().filter_map(|(_, clip)| clip.link.map(|l| l.0))),
        }
    }

    /// Every clip with its track.
    pub(crate) fn clips(&self) -> impl Iterator<Item = (&Track, &Clip)> {
        self.sequence
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter().map(move |clip| (track, clip)))
    }
}

/// Hands out ids that were unused in the project it came from.
#[derive(Debug)]
pub struct FreshIds {
    media: u64,
    clip: u64,
    link: u64,
}

impl FreshIds {
    /// A new media id.
    pub fn media(&mut self) -> MediaId {
        MediaId(take(&mut self.media))
    }

    /// A new clip id.
    pub fn clip(&mut self) -> ClipId {
        ClipId(take(&mut self.clip))
    }

    /// A new link id.
    pub fn link(&mut self) -> LinkId {
        LinkId(take(&mut self.link))
    }
}

fn take(next: &mut u64) -> u64 {
    let id = *next;
    *next += 1;
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(num: u32, den: u32) -> Rational {
        Rational::new(num, den).unwrap()
    }

    fn clip_at(position: i64, source_in_us: i64, speed: f64) -> Clip {
        let mut clip = Clip::new(
            ClipId(1),
            MediaId(1),
            TrackKind::Video,
            (MediaTime(source_in_us), MediaTime(source_in_us + 4_000_000)),
            Frame(position),
            rate(30, 1),
        );
        clip.speed = speed;
        clip
    }

    #[test]
    fn a_new_clip_takes_its_length_from_the_source_range() {
        let clip = clip_at(10, 1_000_000, 1.0);
        assert_eq!(clip.length, Frame(120));
        assert_eq!(clip.end(), Frame(130));
        assert_eq!(clip.kind(), TrackKind::Video);
        assert!(clip.enabled && clip.link.is_none() && clip.speed == 1.0);
    }

    #[test]
    fn the_source_time_follows_the_timeline_frame() {
        let clip = clip_at(10, 1_000_000, 1.0);
        assert_eq!(
            clip.source_time_at(Frame(10), rate(30, 1)),
            MediaTime(1_000_000)
        );
        assert_eq!(
            clip.source_time_at(Frame(40), rate(30, 1)),
            MediaTime(2_000_000)
        );
        let fast = clip_at(10, 1_000_000, 2.0);
        assert_eq!(
            fast.source_time_at(Frame(40), rate(30, 1)),
            MediaTime(3_000_000)
        );
    }

    #[test]
    fn a_new_project_has_one_video_track_and_one_audio_track() {
        let project = Project::new(rate(30000, 1001), (1920, 1080));
        let kinds: Vec<_> = project
            .sequence()
            .tracks()
            .iter()
            .map(Track::kind)
            .collect();
        assert_eq!(kinds, [TrackKind::Video, TrackKind::Audio]);
        assert_eq!(project.sequence().frame_rate(), rate(30000, 1001));
        assert_eq!(project.sequence().resolution(), (1920, 1080));
        assert!(project.media().is_empty());
        let ids: Vec<_> = project.sequence().tracks().iter().map(Track::id).collect();
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn the_visible_video_clip_is_the_enabled_one_on_the_top_unmuted_track() {
        let mut project = Project::new(rate(30, 1), (1920, 1080));
        let low = clip_at(0, 0, 1.0); // frames 0..120
        let mut high = clip_at(60, 0, 1.0); // frames 60..180
        high.id = ClipId(2);
        let mut upper = project.sequence.tracks[0].clone();
        upper.id = TrackId(3);
        upper.clips = vec![high];
        project.sequence.tracks[0].clips.push(low);
        project.sequence.tracks.insert(1, upper);
        let shown = |project: &Project, frame| {
            project
                .sequence()
                .visible_video_at(Frame(frame))
                .map(|clip| clip.id)
        };

        assert_eq!(shown(&project, 10), Some(ClipId(1)));
        assert_eq!(shown(&project, 100), Some(ClipId(2)));
        assert_eq!(shown(&project, 150), Some(ClipId(2)));
        assert_eq!(shown(&project, 180), None);
        project.sequence.tracks[1].muted = true;
        assert_eq!(shown(&project, 100), Some(ClipId(1)));
        project.sequence.tracks[0].clips[0].enabled = false;
        assert_eq!(shown(&project, 100), None);
    }

    #[test]
    fn the_sequence_ends_after_its_last_clip() {
        let mut project = Project::new(rate(30, 1), (1920, 1080));
        assert_eq!(project.sequence().end(), Frame(0));
        project.sequence.tracks[0].clips.push(clip_at(10, 0, 1.0));
        let mut audio = clip_at(200, 0, 1.0);
        audio.edits = ClipEdits::Audio(AudioEdits::default());
        project.sequence.tracks[1].clips.push(audio);
        assert_eq!(project.sequence().end(), Frame(320));
    }

    #[test]
    fn fresh_ids_are_unused_and_distinct() {
        let mut project = Project::new(rate(30, 1), (1920, 1080));
        let mut clip = clip_at(0, 0, 1.0);
        clip.id = ClipId(7);
        clip.link = Some(LinkId(3));
        project.sequence.tracks[0].clips.push(clip);
        let mut ids = project.fresh_ids();
        let (a, b) = (ids.clip(), ids.clip());
        assert!(a.0 > 7 && b.0 > 7 && a != b);
        assert!(ids.link().0 > 3);
        let _ = ids.media();
    }

    #[test]
    fn the_link_group_holds_every_linked_clip() {
        let mut project = Project::new(rate(30, 1), (1920, 1080));
        let mut video = clip_at(0, 0, 1.0);
        video.link = Some(LinkId(1));
        let mut audio = video.clone();
        audio.id = ClipId(2);
        audio.edits = ClipEdits::Audio(AudioEdits::default());
        let mut alone = clip_at(500, 0, 1.0);
        alone.id = ClipId(3);
        project.sequence.tracks[0].clips.extend([video, alone]);
        project.sequence.tracks[1].clips.push(audio);

        let mut group = project.link_group(ClipId(2));
        group.sort();
        assert_eq!(group, [ClipId(1), ClipId(2)]);
        assert_eq!(project.link_group(ClipId(3)), [ClipId(3)]);
        assert!(project.link_group(ClipId(99)).is_empty());
        let (track, clip) = project.find_clip(ClipId(2)).unwrap();
        assert_eq!((track.kind(), clip.id), (TrackKind::Audio, ClipId(2)));
    }
}
