//! Edits as commands (docs/ARCHITECTURE.md, "Commands"). A command either changes the
//! project and keeps every timeline invariant, or leaves the project untouched and returns a
//! [`Rejection`] that says why. An applied command can be reverted exactly.

use std::fmt;

mod clip;
mod edits;
mod insert;
mod moves;
mod remove;
mod sequence;
mod split;
#[cfg(test)]
mod testing;
mod track;
mod trim;

use crate::model::{
    AudioEdits, Clip, ClipEdits, ClipId, MediaId, MediaKind, MediaRef, Project, TrackId, TrackKind,
    VideoEdits,
};
use crate::time::{Frame, MediaTime, length_for};

pub use clip::{SetClipEnabled, Unlink};
pub use edits::{SetAudioEdits, SetVideoEdits};
pub use insert::InsertClips;
pub use moves::{MoveClips, nearest_free_position};
pub use remove::RemoveClips;
pub use sequence::{SEQUENCE_SIDES, SetSequenceSettings};
pub use split::SplitClips;
pub use track::{SetTrackLocked, SetTrackMuted};
pub use trim::{Edge, TrimClips};

/// One edit of a project.
#[derive(Clone, Debug)]
pub enum Command {
    /// Adds a media file to the project.
    AddMedia(MediaRef),
    /// Places clips on tracks.
    InsertClips(InsertClips),
    /// Moves the start or the end of a clip and its linked partners.
    TrimClips(TrimClips),
    /// Moves a clip and its linked partners along the timeline.
    MoveClips(MoveClips),
    /// Splits a clip and its linked partners at a frame.
    SplitClips(SplitClips),
    /// Deletes a clip and its linked partners, with or without ripple.
    RemoveClips(RemoveClips),
    /// Sets the picture edits of a video clip.
    SetVideoEdits(SetVideoEdits),
    /// Sets the sound edits of an audio clip.
    SetAudioEdits(SetAudioEdits),
    /// Enables or disables one clip.
    SetClipEnabled(SetClipEnabled),
    /// Separates the clips of a link group.
    Unlink(Unlink),
    /// Locks or unlocks a track.
    SetTrackLocked(SetTrackLocked),
    /// Mutes or unmutes a track.
    SetTrackMuted(SetTrackMuted),
    /// Sets the sequence's frame rate and picture size.
    SetSequenceSettings(SetSequenceSettings),
    /// Several commands applied as one: all of them, or none.
    Batch(Vec<Command>),
}

impl Command {
    /// Applies the command, or leaves `project` untouched and says why not.
    pub fn apply(&mut self, project: &mut Project) -> Result<(), Rejection> {
        match self {
            Command::AddMedia(media) => {
                if project.media_ref(media.id).is_some() {
                    return Err(Rejection::DuplicateId);
                }
                project.media.push(media.clone());
                Ok(())
            }
            Command::InsertClips(insert) => insert.apply(project),
            Command::TrimClips(trim) => trim.apply(project),
            Command::MoveClips(moves) => moves.apply(project),
            Command::SplitClips(split) => split.apply(project),
            Command::RemoveClips(remove) => remove.apply(project),
            Command::SetVideoEdits(set) => set.apply(project),
            Command::SetAudioEdits(set) => set.apply(project),
            Command::SetClipEnabled(enable) => enable.apply(project),
            Command::Unlink(unlink) => unlink.apply(project),
            Command::SetTrackLocked(lock) => lock.apply(project),
            Command::SetTrackMuted(mute) => mute.apply(project),
            Command::SetSequenceSettings(settings) => settings.apply(project),
            Command::Batch(commands) => {
                for applied in 0..commands.len() {
                    if let Err(rejection) = commands[applied].apply(project) {
                        for command in commands[..applied].iter_mut().rev() {
                            command.revert(project);
                        }
                        return Err(rejection);
                    }
                }
                Ok(())
            }
        }
    }

    /// Undoes the command, which must be the last one applied to `project`.
    pub fn revert(&mut self, project: &mut Project) {
        match self {
            Command::AddMedia(media) => project.media.retain(|other| other.id != media.id),
            Command::InsertClips(insert) => insert.revert(project),
            Command::TrimClips(trim) => trim.revert(project),
            Command::MoveClips(moves) => moves.revert(project),
            Command::SplitClips(split) => split.revert(project),
            Command::RemoveClips(remove) => remove.revert(project),
            Command::SetVideoEdits(set) => set.revert(project),
            Command::SetAudioEdits(set) => set.revert(project),
            Command::SetClipEnabled(enable) => enable.revert(project),
            Command::Unlink(unlink) => unlink.revert(project),
            Command::SetTrackLocked(lock) => lock.revert(project),
            Command::SetTrackMuted(mute) => mute.revert(project),
            Command::SetSequenceSettings(settings) => settings.revert(project),
            Command::Batch(commands) => {
                for command in commands.iter_mut().rev() {
                    command.revert(project);
                }
            }
        }
    }

    /// What applying the command did beyond what was asked, for the UI to tell the user.
    pub fn notices(&self) -> Vec<Notice> {
        match self {
            Command::TrimClips(trim) => trim.notices.clone(),
            Command::SetSequenceSettings(settings) => settings.notices.clone(),
            Command::Batch(commands) => commands.iter().flat_map(Command::notices).collect(),
            Command::AddMedia(_)
            | Command::InsertClips(_)
            | Command::MoveClips(_)
            | Command::SplitClips(_)
            | Command::RemoveClips(_)
            | Command::SetVideoEdits(_)
            | Command::SetAudioEdits(_)
            | Command::SetClipEnabled(_)
            | Command::Unlink(_)
            | Command::SetTrackLocked(_)
            | Command::SetTrackMuted(_) => Vec::new(),
        }
    }
}

/// The track and clip index of clip `id`.
pub(crate) fn clip_indices(project: &Project, id: ClipId) -> Result<(usize, usize), Rejection> {
    project
        .sequence
        .tracks
        .iter()
        .enumerate()
        .find_map(|(t, track)| {
            let i = track.clips.iter().position(|clip| clip.id == id)?;
            Some((t, i))
        })
        .ok_or(Rejection::UnknownClip(id))
}

/// The track and clip indices of `id` and every clip linked to it.
pub(crate) fn group_indices(
    project: &Project,
    id: ClipId,
) -> Result<Vec<(usize, usize)>, Rejection> {
    let group = project.link_group(id);
    if group.is_empty() {
        return Err(Rejection::UnknownClip(id));
    }
    let mut indices = Vec::with_capacity(group.len());
    for (t, track) in project.sequence.tracks.iter().enumerate() {
        for (i, clip) in track.clips.iter().enumerate() {
            if group.contains(&clip.id) {
                indices.push((t, i));
            }
        }
    }
    Ok(indices)
}

/// Where the clip after clip `i` of track `t` starts, if there is one.
pub(crate) fn next_start(project: &Project, t: usize, i: usize) -> Option<Frame> {
    project.sequence.tracks[t]
        .clips
        .get(i + 1)
        .map(|clip| clip.position)
}

/// Where the clip before clip `i` of track `t` ends, if there is one.
pub(crate) fn previous_end(project: &Project, t: usize, i: usize) -> Option<Frame> {
    let previous = i.checked_sub(1)?;
    project.sequence.tracks[t]
        .clips
        .get(previous)
        .map(Clip::end)
}

/// The checks every clip passes, wherever it is on the timeline.
pub(crate) fn check_clip(project: &Project, clip: &Clip) -> Result<(), Rejection> {
    let media = project
        .media_ref(clip.media_id)
        .ok_or(Rejection::UnknownMedia(clip.media_id))?;
    if media.info.kind == MediaKind::Still {
        // A still shows one picture for as long as it lasts: no source range, no speed.
        if clip.kind() != TrackKind::Video {
            return Err(Rejection::WrongTrackKind(clip.id));
        }
        if clip.source_in != MediaTime(0) || clip.source_out != MediaTime(0) {
            return Err(Rejection::SourceRange(clip.id));
        }
        if clip.speed != 1.0 {
            return Err(Rejection::Speed(clip.id));
        }
        if clip.length < Frame(1) {
            return Err(Rejection::TooShort(clip.id));
        }
        if clip.position < Frame(0) {
            return Err(Rejection::BeforeStart(clip.id));
        }
        return match &clip.edits {
            ClipEdits::Video(edits) => check_video_edits(project, clip, edits),
            ClipEdits::Audio(_) => Err(Rejection::WrongTrackKind(clip.id)),
        };
    }
    if clip.source_in < MediaTime(0)
        || clip.source_in >= clip.source_out
        || clip.source_out > media.info.duration
    {
        return Err(Rejection::SourceRange(clip.id));
    }
    if !(MIN_SPEED..=MAX_SPEED).contains(&clip.speed) {
        return Err(Rejection::Speed(clip.id));
    }
    let rate = project.sequence.frame_rate;
    if clip.length != length_for(clip.source_out - clip.source_in, clip.speed, rate) {
        return Err(Rejection::Length(clip.id));
    }
    if clip.position < Frame(0) {
        return Err(Rejection::BeforeStart(clip.id));
    }
    match &clip.edits {
        ClipEdits::Video(edits) => check_video_edits(project, clip, edits),
        ClipEdits::Audio(edits) => check_audio_edits(clip, edits),
    }
}

/// Picture edits `edits` fit `clip`: a crop lies inside the picture.
pub(crate) fn check_video_edits(
    project: &Project,
    clip: &Clip,
    edits: &VideoEdits,
) -> Result<(), Rejection> {
    let Some(crop) = edits.crop else {
        return Ok(());
    };
    let (width, height) = project
        .media_ref(clip.media_id)
        .map_or((0, 0), |media| (media.info.width, media.info.height));
    let inside = crop.width > 0
        && crop.height > 0
        && u64::from(crop.x) + u64::from(crop.width) <= u64::from(width)
        && u64::from(crop.y) + u64::from(crop.height) <= u64::from(height);
    if inside {
        Ok(())
    } else {
        Err(Rejection::Crop(clip.id))
    }
}

/// Sound edits `edits` fit `clip`: a volume in range, and fades that fit inside it.
pub(crate) fn check_audio_edits(clip: &Clip, edits: &AudioEdits) -> Result<(), Rejection> {
    if !AudioEdits::VOLUME_RANGE.contains(&edits.volume_db) {
        return Err(Rejection::Volume(clip.id));
    }
    let (fade_in, fade_out) = (edits.fade_in, edits.fade_out);
    if fade_in < Frame(0) || fade_out < Frame(0) || fade_in + fade_out > clip.length {
        return Err(Rejection::Fades(clip.id));
    }
    Ok(())
}

pub(crate) fn overlaps(a: &Clip, b: &Clip) -> bool {
    a.position < b.end() && b.position < a.end()
}

pub(crate) fn same_timing(a: &Clip, b: &Clip) -> bool {
    (a.source_in, a.source_out, a.position, a.length)
        == (b.source_in, b.source_out, b.position, b.length)
        && a.speed == b.speed
}

/// The slowest and fastest clip speeds (docs/ARCHITECTURE.md, "Core data model").
const MIN_SPEED: f64 = 0.1;
const MAX_SPEED: f64 = 32.0;

/// Why a command was refused. The project is unchanged.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    /// The command names a track the sequence does not have.
    #[error("that track no longer exists; reopen the view and try again")]
    UnknownTrack(TrackId),
    /// The command names a clip the project does not have.
    #[error("that clip no longer exists; reopen the view and try again")]
    UnknownClip(ClipId),
    /// A clip refers to a media file the project does not have.
    #[error("the clip's media file is not in the project; import it first")]
    UnknownMedia(MediaId),
    /// A new media file or clip reuses an id.
    #[error("an item with the same id is already in the project")]
    DuplicateId,
    /// The edit would change a clip on a locked track.
    #[error("a track this edit touches is locked; unlock the track or unlink the clips")]
    TrackLocked(TrackId),
    /// A video clip on an audio track, or the other way round.
    #[error("video clips go on video tracks and audio clips on audio tracks")]
    WrongTrackKind(ClipId),
    /// The edit would make two clips on a track overlap.
    #[error("the clip would overlap another clip; make room first")]
    Overlap(TrackId),
    /// A clip's source range does not lie within its media file.
    #[error("the clip reaches outside its source file")]
    SourceRange(ClipId),
    /// The edit would leave a clip shorter than one frame.
    #[error("a clip must be at least one frame long")]
    TooShort(ClipId),
    /// The edit would move a clip before the start of the timeline.
    #[error("a clip cannot start before the beginning of the timeline")]
    BeforeStart(ClipId),
    /// A clip's speed is outside 0.1x to 32x.
    #[error("speed must be between 0.1x and 32x")]
    Speed(ClipId),
    /// A clip's length does not follow from its source range and speed.
    #[error("the clip's length does not match its source range and speed")]
    Length(ClipId),
    /// Linked clips differ in source range, position, length or speed.
    #[error("linked clips must share their source range, position, length and speed")]
    LinkMismatch(ClipId),
    /// The clip is not linked to another clip.
    #[error("that clip is not linked to another clip")]
    NotLinked(ClipId),
    /// Volume and fades were given to a video clip.
    #[error("volume and fades apply to audio clips")]
    NotAudio(ClipId),
    /// Fit, crop or rotation were given to an audio clip.
    #[error("fit, crop and rotation apply to video clips")]
    NotVideo(ClipId),
    /// The fades are negative or longer together than the clip.
    #[error("the fades must fit inside the clip; shorten them first")]
    Fades(ClipId),
    /// The volume is outside the range a clip can be set to.
    #[error("the volume must be between -60 and +12 dB")]
    Volume(ClipId),
    /// The crop is empty or reaches outside the picture.
    #[error("the crop must lie inside the picture")]
    Crop(ClipId),
    /// A split was asked for at a frame that is not strictly inside the clip.
    #[error("move the playhead inside the clip to split it")]
    SplitOutside(ClipId),
    /// The sequence size is outside what Dusk supports.
    #[error("the sequence must be 16 to 8192 pixels on each side")]
    Resolution,
    /// A ripple delete would move clips onto a clip that overlaps the deleted range.
    #[error(
        "a clip on another track overlaps the deleted range; lock that track or use plain delete"
    )]
    RippleBlocked(TrackId),
}

/// Something a command did beyond what was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// An end trim ran into the next clip, so the clip was cut where that clip starts.
    CutAtGap(ClipId),
    /// A trim reached the start or the end of the source file and stopped there.
    ReachedSourceEdge(ClipId),
    /// A new frame rate would have left a clip without a frame, so it kept one and the
    /// clips after it moved right by a frame.
    LengthenedToOneFrame(ClipId),
}

impl fmt::Display for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Notice::CutAtGap(_) => {
                "The clip was cut where the next clip starts; ripple the timeline to make room."
            }
            Notice::ReachedSourceEdge(_) => "The clip reached the edge of its source file.",
            Notice::LengthenedToOneFrame(_) => {
                "A clip shorter than a frame at the new rate kept one frame; the clips after it \
                 moved right by a frame."
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::*;
    use crate::model::TrackKind;
    use crate::time::MediaTime;

    #[test]
    fn adding_media_lists_it_once_and_reverts() {
        let empty = Project::new(fps30(), (1920, 1080));
        let mut project = empty.clone();
        let media = project_media();
        let mut add = Command::AddMedia(media.clone());
        add.apply(&mut project).unwrap();
        assert_eq!(project.media(), std::slice::from_ref(&media));
        assert_eq!(
            Command::AddMedia(media).apply(&mut project),
            Err(Rejection::DuplicateId)
        );
        add.revert(&mut project);
        assert_eq!(project, empty);
    }

    fn project_media() -> MediaRef {
        project().media()[0].clone()
    }

    #[test]
    fn a_batch_applies_all_or_nothing_and_reverts_in_reverse() {
        let empty = Project::new(fps30(), (1920, 1080));
        let media = project_media();
        let video_track = track(&empty, TrackKind::Video);
        let clip = Clip::new(
            ClipId(1),
            MediaId(1),
            TrackKind::Video,
            (MediaTime(0), MediaTime(SECOND)),
            Frame(0),
            fps30(),
        );
        let mut bad = clip.clone();
        bad.length = Frame(2);

        let mut project = empty.clone();
        let mut refused = Command::Batch(vec![
            Command::AddMedia(media.clone()),
            Command::InsertClips(InsertClips::new(vec![(video_track, bad)])),
        ]);
        assert_eq!(
            refused.apply(&mut project),
            Err(Rejection::Length(ClipId(1)))
        );
        assert_eq!(project, empty);

        let mut import = Command::Batch(vec![
            Command::AddMedia(media),
            Command::InsertClips(InsertClips::new(vec![(video_track, clip)])),
        ]);
        import.apply(&mut project).unwrap();
        assert_eq!(project.sequence().tracks()[0].clips().len(), 1);
        import.revert(&mut project);
        assert_eq!(project, empty);
    }
}
