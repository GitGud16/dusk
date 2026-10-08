//! The project file (docs/ARCHITECTURE.md, "Project file and autosave"): pretty-printed JSON
//! that states its format version and lists each media file once, referred to by id. Media
//! in the project file's folder, or below it, is saved relative to that folder with `/`
//! between parts, so the folder can move as a whole; other media keeps its absolute path.
//! Positions, lengths and fades are frames at the sequence rate, source times and durations
//! are microseconds, and rates are fractions such as "30000/1001". A file is checked against
//! every timeline rule when it is read, and one that breaks any is refused, never patched up.

use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::command::{Rejection, SEQUENCE_SIDES, check_clip, overlaps, same_timing};
use crate::model::{
    AudioEdits, Clip, ClipEdits, ClipId, Fit, LinkId, MediaId, MediaInfo, MediaKind, MediaRef,
    Project, Rect, Rotation, Sequence, Track, TrackId, TrackKind, VideoEdits,
};
use crate::orientation::Orientation;
use crate::time::{Frame, MediaTime, Rational};

/// The format version this Dusk writes, and the only one it reads.
pub const FORMAT_VERSION: u32 = 1;

/// Why a project file could not be read.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// The text is not a Dusk project file, or a value in it is out of range.
    #[error("the project file is damaged or is not a Dusk project ({0})")]
    Unreadable(String),
    /// The file is in a format version this Dusk does not read.
    #[error(
        "the project file is in format version {0}, which this Dusk cannot open; a newer Dusk may"
    )]
    Version(u64),
    /// The file breaks a timeline rule.
    #[error("the project file breaks a timeline rule: {0}")]
    Invalid(#[from] Rejection),
}

/// The text of `project` saved as the project file `file`. Media paths are expected to be
/// absolute, as import makes them, and so is `file`.
pub fn to_json(project: &Project, file: &Path) -> String {
    let folder = folder_of(file);
    let saved = SavedProject {
        version: FORMAT_VERSION,
        media: project
            .media
            .iter()
            .map(|media| saved_media(media, &folder))
            .collect(),
        sequence: saved_sequence(&project.sequence),
    };
    // Structs with string keys always serialize to JSON.
    let mut text = serde_json::to_string_pretty(&saved).expect("a project serializes to JSON");
    text.push('\n');
    text
}

/// The project saved in `text`, read from the project file `file`.
pub fn from_json(text: &str, file: &Path) -> Result<Project, FileError> {
    /// The one field every version has, read first so that a newer file is reported as
    /// newer rather than as damaged.
    #[derive(Deserialize)]
    struct Header {
        version: u64,
    }
    let header: Header = serde_json::from_str(text).map_err(unreadable)?;
    if header.version != u64::from(FORMAT_VERSION) {
        return Err(FileError::Version(header.version));
    }
    let saved: SavedProject = serde_json::from_str(text).map_err(unreadable)?;
    let mut project = loaded_project(saved, &folder_of(file));
    check(&mut project)?;
    Ok(project)
}

fn unreadable(error: impl Display) -> FileError {
    FileError::Unreadable(error.to_string())
}

fn folder_of(file: &Path) -> PathBuf {
    file.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// How a media path is written: relative to `folder`, with `/` between parts, when it lies
/// inside it; as it is otherwise.
fn saved_path(path: &Path, folder: &Path) -> String {
    match path.strip_prefix(folder) {
        Ok(inside) if folder.is_absolute() => {
            let parts: Vec<_> = inside.iter().map(|part| part.to_string_lossy()).collect();
            parts.join("/")
        }
        // Import only takes paths FFmpeg can open, which are Unicode, so nothing is lost.
        _ => path.to_string_lossy().into_owned(),
    }
}

/// Where a media path written as `saved` points, for a project file in `folder`.
fn loaded_path(saved: &str, folder: &Path) -> PathBuf {
    let path = Path::new(saved);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let mut full = folder.to_path_buf();
    full.extend(saved.split('/'));
    full
}

/// Checks a project just read against every rule the commands keep, after putting each
/// track's clips in timeline order.
fn check(project: &mut Project) -> Result<(), FileError> {
    let (width, height) = project.sequence.resolution;
    if !SEQUENCE_SIDES.contains(&width) || !SEQUENCE_SIDES.contains(&height) {
        return Err(Rejection::Resolution.into());
    }
    let mut media = HashSet::new();
    if !project.media.iter().all(|item| media.insert(item.id)) {
        return Err(Rejection::DuplicateId.into());
    }
    let tracks = &mut project.sequence.tracks;
    let mut track_ids = HashSet::new();
    if let Some(track) = tracks.iter().find(|track| !track_ids.insert(track.id)) {
        return Err(unreadable(format!("two tracks have id {}", track.id.0)));
    }
    let audio_first = tracks
        .windows(2)
        .any(|pair| pair[0].kind == TrackKind::Audio && pair[1].kind == TrackKind::Video);
    if audio_first {
        return Err(unreadable("video tracks must come before audio tracks"));
    }
    for track in tracks.iter_mut() {
        track.clips.sort_by_key(|clip| clip.position);
    }
    let mut clip_ids = HashSet::new();
    for track in &project.sequence.tracks {
        for clip in &track.clips {
            if clip.kind() != track.kind {
                return Err(Rejection::WrongTrackKind(clip.id).into());
            }
            check_clip(project, clip)?;
            if !clip_ids.insert(clip.id) {
                return Err(Rejection::DuplicateId.into());
            }
        }
        if track
            .clips
            .windows(2)
            .any(|pair| overlaps(&pair[0], &pair[1]))
        {
            return Err(Rejection::Overlap(track.id).into());
        }
    }
    let mut groups: HashMap<LinkId, &Clip> = HashMap::new();
    for (_, clip) in project.clips() {
        if let Some(link) = clip.link {
            let first = *groups.entry(link).or_insert(clip);
            if !same_timing(first, clip) {
                return Err(Rejection::LinkMismatch(clip.id).into());
            }
        }
    }
    Ok(())
}

// The file's own types. The format is defined here, apart from the model, so that a change
// to the model cannot change the format unnoticed.

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedProject {
    version: u32,
    media: Vec<SavedMedia>,
    sequence: SavedSequence,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedMedia {
    id: Id,
    path: String,
    kind: SavedMediaKind,
    duration: Count,
    has_video: bool,
    has_audio: bool,
    frame_rate: Option<SavedRate>,
    vfr: bool,
    width: u32,
    height: u32,
    /// The orientation: clockwise turns in degrees, after mirroring left to right.
    rotation: Degrees,
    mirrored: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SavedMediaKind {
    Video,
    Audio,
    Still,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedSequence {
    frame_rate: SavedRate,
    width: u32,
    height: u32,
    tracks: Vec<SavedTrack>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedTrack {
    id: Id,
    kind: SavedTrackKind,
    locked: bool,
    muted: bool,
    clips: Vec<SavedClip>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SavedTrackKind {
    Video,
    Audio,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedClip {
    id: Id,
    media: Id,
    source_in: Count,
    source_out: Count,
    position: Count,
    length: Count,
    speed: f64,
    enabled: bool,
    link: Option<Id>,
    edits: SavedEdits,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SavedEdits {
    Video {
        crop: Option<SavedRect>,
        rotate: Degrees,
        flip_h: bool,
        flip_v: bool,
        fit: SavedFit,
    },
    Audio {
        volume_db: f32,
        fade_in: Count,
        fade_out: Count,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SavedFit {
    Fit,
    Fill,
}

/// The largest whole number the file holds, 2^53: as far as JSON tools count exactly, and
/// small enough that no sum Dusk forms from loaded positions, lengths or ids can overflow.
const LARGEST: i64 = 1 << 53;

/// An id of a media file, track, clip or link group.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
struct Id(u64);

impl TryFrom<u64> for Id {
    type Error = String;
    fn try_from(value: u64) -> Result<Id, String> {
        if value <= LARGEST.unsigned_abs() {
            Ok(Id(value))
        } else {
            Err(format!("id {value} is too large"))
        }
    }
}

impl From<Id> for u64 {
    fn from(id: Id) -> u64 {
        id.0
    }
}

/// A number of frames or microseconds.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
struct Count(i64);

impl TryFrom<i64> for Count {
    type Error = String;
    fn try_from(value: i64) -> Result<Count, String> {
        if (-LARGEST..=LARGEST).contains(&value) {
            Ok(Count(value))
        } else {
            Err(format!("{value} is too large for a timeline"))
        }
    }
}

impl From<Count> for i64 {
    fn from(count: Count) -> i64 {
        count.0
    }
}

/// A frame rate, written as a fraction such as "30000/1001".
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
struct SavedRate(Rational);

impl TryFrom<String> for SavedRate {
    type Error = String;
    fn try_from(text: String) -> Result<SavedRate, String> {
        text.split_once('/')
            .and_then(|(num, den)| Rational::new(num.parse().ok()?, den.parse().ok()?))
            .map(SavedRate)
            .ok_or_else(|| {
                format!("frame rate \"{text}\" is not a fraction of whole numbers such as \"30/1\"")
            })
    }
}

impl From<SavedRate> for String {
    fn from(rate: SavedRate) -> String {
        format!("{}/{}", rate.0.num(), rate.0.den())
    }
}

/// A clockwise rotation, written in degrees.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
struct Degrees(Rotation);

impl TryFrom<u16> for Degrees {
    type Error = String;
    fn try_from(degrees: u16) -> Result<Degrees, String> {
        Ok(Degrees(match degrees {
            0 => Rotation::None,
            90 => Rotation::Quarter,
            180 => Rotation::Half,
            270 => Rotation::ThreeQuarters,
            _ => {
                return Err(format!(
                    "rotation {degrees} is not 0, 90, 180 or 270 degrees"
                ));
            }
        }))
    }
}

impl From<Degrees> for u16 {
    fn from(degrees: Degrees) -> u16 {
        match degrees.0 {
            Rotation::None => 0,
            Rotation::Quarter => 90,
            Rotation::Half => 180,
            Rotation::ThreeQuarters => 270,
        }
    }
}

fn saved_media(media: &MediaRef, folder: &Path) -> SavedMedia {
    let info = &media.info;
    SavedMedia {
        id: Id(media.id.0),
        path: saved_path(&media.path, folder),
        kind: match info.kind {
            MediaKind::Video => SavedMediaKind::Video,
            MediaKind::Audio => SavedMediaKind::Audio,
            MediaKind::Still => SavedMediaKind::Still,
        },
        duration: Count(info.duration.0),
        has_video: info.has_video,
        has_audio: info.has_audio,
        frame_rate: info.frame_rate.map(SavedRate),
        vfr: info.vfr,
        width: info.width,
        height: info.height,
        rotation: Degrees(turns_to_rotation(info.orientation.turns())),
        mirrored: info.orientation.mirrored(),
    }
}

fn rotation_to_turns(rotation: Rotation) -> u8 {
    match rotation {
        Rotation::None => 0,
        Rotation::Quarter => 1,
        Rotation::Half => 2,
        Rotation::ThreeQuarters => 3,
    }
}

fn turns_to_rotation(turns: u8) -> Rotation {
    match turns {
        1 => Rotation::Quarter,
        2 => Rotation::Half,
        3 => Rotation::ThreeQuarters,
        _ => Rotation::None,
    }
}

fn saved_sequence(sequence: &Sequence) -> SavedSequence {
    let track = |track: &Track| SavedTrack {
        id: Id(track.id.0),
        kind: match track.kind {
            TrackKind::Video => SavedTrackKind::Video,
            TrackKind::Audio => SavedTrackKind::Audio,
        },
        locked: track.locked,
        muted: track.muted,
        clips: track.clips.iter().map(saved_clip).collect(),
    };
    SavedSequence {
        frame_rate: SavedRate(sequence.frame_rate),
        width: sequence.resolution.0,
        height: sequence.resolution.1,
        tracks: sequence.tracks.iter().map(track).collect(),
    }
}

fn saved_clip(clip: &Clip) -> SavedClip {
    let edits = match &clip.edits {
        ClipEdits::Video(edits) => SavedEdits::Video {
            crop: edits.crop.map(|crop| SavedRect {
                x: crop.x,
                y: crop.y,
                width: crop.width,
                height: crop.height,
            }),
            rotate: Degrees(edits.rotate),
            flip_h: edits.flip_h,
            flip_v: edits.flip_v,
            fit: match edits.fit {
                Fit::Fit => SavedFit::Fit,
                Fit::Fill => SavedFit::Fill,
            },
        },
        ClipEdits::Audio(edits) => SavedEdits::Audio {
            volume_db: edits.volume_db,
            fade_in: Count(edits.fade_in.0),
            fade_out: Count(edits.fade_out.0),
        },
    };
    SavedClip {
        id: Id(clip.id.0),
        media: Id(clip.media_id.0),
        source_in: Count(clip.source_in.0),
        source_out: Count(clip.source_out.0),
        position: Count(clip.position.0),
        length: Count(clip.length.0),
        speed: clip.speed,
        enabled: clip.enabled,
        link: clip.link.map(|link| Id(link.0)),
        edits,
    }
}

fn loaded_project(saved: SavedProject, folder: &Path) -> Project {
    let sequence = saved.sequence;
    Project {
        media: saved
            .media
            .into_iter()
            .map(|media| loaded_media(media, folder))
            .collect(),
        sequence: Sequence {
            frame_rate: sequence.frame_rate.0,
            resolution: (sequence.width, sequence.height),
            tracks: sequence.tracks.into_iter().map(loaded_track).collect(),
        },
    }
}

fn loaded_media(media: SavedMedia, folder: &Path) -> MediaRef {
    MediaRef {
        id: MediaId(media.id.0),
        path: loaded_path(&media.path, folder),
        info: MediaInfo {
            kind: match media.kind {
                SavedMediaKind::Video => MediaKind::Video,
                SavedMediaKind::Audio => MediaKind::Audio,
                SavedMediaKind::Still => MediaKind::Still,
            },
            duration: MediaTime(media.duration.0),
            has_video: media.has_video,
            has_audio: media.has_audio,
            frame_rate: media.frame_rate.map(|rate| rate.0),
            vfr: media.vfr,
            width: media.width,
            height: media.height,
            orientation: Orientation::new(rotation_to_turns(media.rotation.0), media.mirrored),
        },
    }
}

fn loaded_track(track: SavedTrack) -> Track {
    Track {
        id: TrackId(track.id.0),
        kind: match track.kind {
            SavedTrackKind::Video => TrackKind::Video,
            SavedTrackKind::Audio => TrackKind::Audio,
        },
        locked: track.locked,
        muted: track.muted,
        clips: track.clips.into_iter().map(loaded_clip).collect(),
    }
}

fn loaded_clip(clip: SavedClip) -> Clip {
    let edits = match clip.edits {
        SavedEdits::Video {
            crop,
            rotate,
            flip_h,
            flip_v,
            fit,
        } => ClipEdits::Video(VideoEdits {
            crop: crop.map(|crop| Rect {
                x: crop.x,
                y: crop.y,
                width: crop.width,
                height: crop.height,
            }),
            rotate: rotate.0,
            flip_h,
            flip_v,
            fit: match fit {
                SavedFit::Fit => Fit::Fit,
                SavedFit::Fill => Fit::Fill,
            },
        }),
        SavedEdits::Audio {
            volume_db,
            fade_in,
            fade_out,
        } => ClipEdits::Audio(AudioEdits {
            volume_db,
            fade_in: Frame(fade_in.0),
            fade_out: Frame(fade_out.0),
        }),
    };
    Clip {
        id: ClipId(clip.id.0),
        media_id: MediaId(clip.media.0),
        source_in: MediaTime(clip.source_in.0),
        source_out: MediaTime(clip.source_out.0),
        position: Frame(clip.position.0),
        length: Frame(clip.length.0),
        speed: clip.speed,
        enabled: clip.enabled,
        link: clip.link.map(|link| LinkId(link.0)),
        edits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::*;
    use crate::command::{
        Command, InsertClips, SetAudioEdits, SetTrackLocked, SetTrackMuted, SetVideoEdits,
        SplitClips,
    };
    use crate::time::length_for;

    fn folder() -> PathBuf {
        std::env::temp_dir().join("dusk-file-tests")
    }

    fn file() -> PathBuf {
        folder().join("edit.dusk")
    }

    fn reload(project: &Project) -> Result<Project, FileError> {
        from_json(&to_json(project, &file()), &file())
    }

    /// A project that uses every part of the format. Its video sits beside the project file,
    /// its photo elsewhere.
    fn rich_project() -> Project {
        let mut project = project();
        project.media[0].path = folder().join("clip.mp4");
        let (video, audio) = insert_pair(&mut project, 0, (SECOND, 5 * SECOND));
        let photo = add_still(&mut project);
        project.media[1].path = std::env::temp_dir().join("photos").join("photo.jpg");
        insert_still(&mut project, photo, 200, 75);
        let audio_track = track(&project, TrackKind::Audio);
        let v2 = project.sequence().tracks()[1].id();
        let mut commands = vec![
            Command::SplitClips(SplitClips::new(video, Frame(60))),
            Command::SetAudioEdits(SetAudioEdits::new(
                audio,
                AudioEdits {
                    volume_db: -4.5,
                    fade_in: Frame(10),
                    fade_out: Frame(0),
                },
            )),
            Command::SetVideoEdits(SetVideoEdits::new(
                video,
                VideoEdits {
                    crop: Some(Rect {
                        x: 10,
                        y: 20,
                        width: 640,
                        height: 360,
                    }),
                    rotate: Rotation::Quarter,
                    flip_h: true,
                    flip_v: false,
                    fit: Fit::Fill,
                },
            )),
            Command::SetTrackMuted(SetTrackMuted::new(audio_track, true)),
            Command::SetTrackLocked(SetTrackLocked::new(v2, true)),
        ];
        for command in &mut commands {
            command.apply(&mut project).unwrap();
        }
        project
    }

    #[test]
    fn a_project_survives_saving_and_loading() {
        let project = rich_project();
        assert_eq!(reload(&project).unwrap(), project);
    }

    #[test]
    fn the_file_reads_by_eye() {
        let text = to_json(&rich_project(), &file());
        assert!(text.starts_with("{\n  \"version\": 1,\n"), "{text}");
        assert!(text.contains("\"frame_rate\": \"30/1\""), "{text}");
        assert!(text.contains("\"rotate\": 90"), "{text}");
        assert!(text.ends_with("}\n"));
    }

    /// Version 1 of the format, as Dusk writes it.
    const VERSION_1: &str = r#"{
  "version": 1,
  "media": [
    {
      "id": 1,
      "path": "clips/beach.mp4",
      "kind": "video",
      "duration": 10000000,
      "has_video": true,
      "has_audio": true,
      "frame_rate": "30000/1001",
      "vfr": false,
      "width": 1920,
      "height": 1080,
      "rotation": 0,
      "mirrored": false
    }
  ],
  "sequence": {
    "frame_rate": "30000/1001",
    "width": 1920,
    "height": 1080,
    "tracks": [
      {
        "id": 1,
        "kind": "video",
        "locked": false,
        "muted": false,
        "clips": [
          {
            "id": 1,
            "media": 1,
            "source_in": 0,
            "source_out": 2002000,
            "position": 0,
            "length": 60,
            "speed": 1.0,
            "enabled": true,
            "link": 1,
            "edits": {
              "kind": "video",
              "crop": null,
              "rotate": 0,
              "flip_h": false,
              "flip_v": false,
              "fit": "fit"
            }
          }
        ]
      },
      {
        "id": 2,
        "kind": "audio",
        "locked": false,
        "muted": true,
        "clips": [
          {
            "id": 2,
            "media": 1,
            "source_in": 0,
            "source_out": 2002000,
            "position": 0,
            "length": 60,
            "speed": 1.0,
            "enabled": true,
            "link": 1,
            "edits": {
              "kind": "audio",
              "volume_db": -3.0,
              "fade_in": 15,
              "fade_out": 0
            }
          }
        ]
      }
    ]
  }
}
"#;

    #[test]
    fn version_1_files_read_and_write_the_same_way() {
        // Git may check this source out with Windows line ends.
        let text = VERSION_1.replace("\r\n", "\n");
        let folder = std::env::temp_dir().join("golden");
        let file = folder.join("edit.dusk");
        let rate = Rational::new(30000, 1001).unwrap();
        let source = (MediaTime(0), MediaTime(2_002_000));
        let mut video = Clip::new(
            ClipId(1),
            MediaId(1),
            TrackKind::Video,
            source,
            Frame(0),
            rate,
        );
        video.link = Some(LinkId(1));
        let mut audio = Clip::new(
            ClipId(2),
            MediaId(1),
            TrackKind::Audio,
            source,
            Frame(0),
            rate,
        );
        audio.link = Some(LinkId(1));
        audio.edits = ClipEdits::Audio(AudioEdits {
            volume_db: -3.0,
            fade_in: Frame(15),
            fade_out: Frame(0),
        });
        let track = |id, kind, muted, clip| Track {
            id: TrackId(id),
            kind,
            locked: false,
            muted,
            clips: vec![clip],
        };
        let expected = Project {
            media: vec![MediaRef {
                id: MediaId(1),
                path: folder.join("clips").join("beach.mp4"),
                info: MediaInfo {
                    kind: MediaKind::Video,
                    duration: MediaTime(10_000_000),
                    has_video: true,
                    has_audio: true,
                    frame_rate: Some(rate),
                    vfr: false,
                    width: 1920,
                    height: 1080,
                    orientation: crate::orientation::Orientation::UPRIGHT,
                },
            }],
            sequence: Sequence {
                frame_rate: rate,
                resolution: (1920, 1080),
                tracks: vec![
                    track(1, TrackKind::Video, false, video),
                    track(2, TrackKind::Audio, true, audio),
                ],
            },
        };
        assert_eq!(from_json(&text, &file).unwrap(), expected);
        assert_eq!(to_json(&expected, &file), text);
    }

    #[test]
    fn media_beside_the_project_moves_with_it() {
        let project = rich_project();
        let text = to_json(&project, &file());
        assert!(text.contains("\"path\": \"clip.mp4\""), "{text}");
        let moved = std::env::temp_dir().join("moved").join("edit.dusk");
        let loaded = from_json(&text, &moved).unwrap();
        let path = |project: &Project, id| project.media_ref(id).unwrap().path.clone();
        let photo = MediaId(2);
        assert_eq!(
            path(&loaded, MediaId(1)),
            std::env::temp_dir().join("moved").join("clip.mp4")
        );
        assert_eq!(path(&loaded, photo), path(&project, photo));
    }

    #[test]
    fn media_in_a_subfolder_is_saved_with_forward_slashes() {
        let mut project = rich_project();
        project.media[0].path = folder().join("footage").join("day 1").join("clip.mp4");
        let text = to_json(&project, &file());
        assert!(
            text.contains("\"path\": \"footage/day 1/clip.mp4\""),
            "{text}"
        );
        assert_eq!(reload(&project).unwrap(), project);
    }

    #[test]
    fn every_speed_reads_back_to_the_last_bit() {
        let mut project = project();
        project.media[0].path = folder().join("clip.mp4");
        let clip = Clip::new(
            ClipId(1),
            MediaId(1),
            TrackKind::Video,
            (MediaTime(0), MediaTime(3 * SECOND)),
            Frame(0),
            fps30(),
        );
        let video = track(&project, TrackKind::Video);
        Command::InsertClips(InsertClips::new(vec![(video, clip)]))
            .apply(&mut project)
            .unwrap();
        for step in 0..2_000 {
            let speed = 0.1 + f64::from(step) * 0.015_707_963_267_948_967;
            let clip = &mut project.sequence.tracks[0].clips[0];
            clip.speed = speed;
            clip.length = length_for(clip.source_out - clip.source_in, speed, fps30());
            assert_eq!(reload(&project).unwrap(), project, "speed {speed}");
        }
    }

    #[test]
    fn clips_listed_out_of_order_load_in_order() {
        let project = rich_project();
        let mut shuffled = project.clone();
        shuffled.sequence.tracks[0].clips.reverse();
        assert_eq!(reload(&shuffled).unwrap(), project);
    }

    fn unreadable_because(text: &str) -> String {
        match from_json(text, &file()) {
            Err(FileError::Unreadable(why)) => why,
            other => panic!("expected an unreadable file, got {other:?}"),
        }
    }

    #[test]
    fn text_that_is_not_a_project_is_refused() {
        unreadable_because("{ not json");
        unreadable_because("[1, 2, 3]");
        unreadable_because("{\"version\": 1}");
        let text = to_json(&rich_project(), &file());
        let why = unreadable_because(&text.replace("\"vfr\": false", "\"vfr\": \"no\""));
        assert!(why.contains("line"), "{why}");
        unreadable_because(&text.replacen("\"vfr\": false", "\"vfr\": false, \"hue\": 3", 1));
    }

    #[test]
    fn a_file_from_a_newer_dusk_says_so() {
        let text = to_json(&rich_project(), &file());
        let newer = text.replace("\"version\": 1", "\"version\": 2");
        assert!(matches!(
            from_json(&newer, &file()),
            Err(FileError::Version(2))
        ));
        let newer = "{\"version\": 9, \"something\": \"else entirely\"}";
        assert!(matches!(
            from_json(newer, &file()),
            Err(FileError::Version(9))
        ));
    }

    #[test]
    fn values_dusk_cannot_use_are_refused() {
        let text = to_json(&rich_project(), &file());
        let why = unreadable_because(&text.replacen("\"30/1\"", "\"0/1\"", 1));
        assert!(why.contains("0/1"), "{why}");
        unreadable_because(&text.replacen("\"30/1\"", "\"thirty\"", 1));
        let why = unreadable_because(&text.replace("\"rotate\": 90", "\"rotate\": 45"));
        assert!(why.contains("45"), "{why}");
        // Far beyond any timeline: sums of such numbers could overflow.
        let mut project = rich_project();
        project.sequence.tracks[0].clips[2].position = Frame(1 << 60);
        unreadable_because(&to_json(&project, &file()));
        let mut project = rich_project();
        project.sequence.tracks[0].clips[2].id = ClipId(u64::MAX);
        unreadable_because(&to_json(&project, &file()));
    }

    #[test]
    fn tracks_must_be_distinct_with_video_first() {
        let mut project = rich_project();
        project.sequence.tracks.swap(1, 2);
        unreadable_because(&to_json(&project, &file()));
        let mut project = rich_project();
        project.sequence.tracks[3].id = project.sequence.tracks[2].id;
        unreadable_because(&to_json(&project, &file()));
    }

    fn breaks(project: &Project) -> Rejection {
        match reload(project) {
            Err(FileError::Invalid(rejection)) => rejection,
            other => panic!("expected a broken timeline rule, got {other:?}"),
        }
    }

    #[test]
    fn a_file_that_breaks_a_timeline_rule_is_refused() {
        // V1 holds the left and right video halves and the photo, A1 the two audio halves.
        let rich = rich_project();
        let v1 = rich.sequence().tracks()[0].id();
        let ids = |t: usize| -> Vec<ClipId> {
            rich.sequence().tracks()[t]
                .clips()
                .iter()
                .map(|clip| clip.id)
                .collect()
        };
        let (video, audio) = (ids(0), ids(2));

        let mut project = rich.clone();
        project.sequence.tracks[0].clips[1].position = Frame(10);
        assert_eq!(breaks(&project), Rejection::Overlap(v1));

        let mut project = rich.clone();
        project.media.retain(|media| media.id != MediaId(1));
        assert_eq!(breaks(&project), Rejection::UnknownMedia(MediaId(1)));

        let mut project = rich.clone();
        project.sequence.tracks[2].clips[1].position = Frame(70);
        assert_eq!(breaks(&project), Rejection::LinkMismatch(audio[1]));

        let mut project = rich.clone();
        project.sequence.tracks[0].clips[2].id = video[0];
        assert_eq!(breaks(&project), Rejection::DuplicateId);

        let mut project = rich.clone();
        let photo = project.media[1].clone();
        project.media.push(photo);
        assert_eq!(breaks(&project), Rejection::DuplicateId);

        let mut project = rich.clone();
        let mut stray = project.sequence.tracks[2].clips[0].clone();
        stray.id = ClipId(99);
        stray.link = None;
        project.sequence.tracks[1].clips.push(stray);
        assert_eq!(breaks(&project), Rejection::WrongTrackKind(ClipId(99)));

        let mut project = rich.clone();
        project.sequence.tracks[0].clips[0].length = Frame(59);
        assert_eq!(breaks(&project), Rejection::Length(video[0]));

        let mut project = rich.clone();
        project.sequence.resolution = (1920, 9000);
        assert_eq!(breaks(&project), Rejection::Resolution);
    }
}
