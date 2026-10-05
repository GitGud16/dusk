//! The Dusk project model: project, sequence, tracks and clips and their edits as plain data,
//! the commands that change them, and their invariants.
//!
//! Depends only on `serde`; no FFmpeg, wgpu, Slint or I/O (see docs/ARCHITECTURE.md).

mod command;
pub mod file;
mod import;
mod model;
mod picture;
pub mod time;

pub use command::{
    Command, Edge, InsertClips, MoveClips, Notice, Rejection, RemoveClips, SEQUENCE_SIDES,
    SetAudioEdits, SetClipEnabled, SetSequenceSettings, SetTrackLocked, SetTrackMuted,
    SetVideoEdits, SplitClips, TrimClips, Unlink, nearest_free_position, remove_one, split_at,
};
pub use import::{STILL_LENGTH, add_media, import, nearest_free_place, place, place_where_free};
pub use model::{
    AudioEdits, Clip, ClipEdits, ClipId, Fit, FreshIds, LinkId, MediaId, MediaInfo, MediaKind,
    MediaRef, Project, Rect, Rotation, Sequence, Track, TrackId, TrackKind, VideoEdits,
};
pub use picture::{ColorMatrix, ColorRange, Picture, PictureLayout};
pub use time::{Frame, MediaTime, Rational};
