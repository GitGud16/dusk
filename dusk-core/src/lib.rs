//! The Dusk project model: project, sequence, tracks and clips and their edits as plain data,
//! the commands that change them, and their invariants.
//!
//! Depends only on `serde`; no FFmpeg, wgpu, Slint or I/O (see docs/ARCHITECTURE.md).

mod command;
mod import;
mod model;
mod picture;
pub mod time;

pub use command::{
    Command, Edge, InsertClips, Notice, Rejection, SetAudioEdits, SetClipEnabled, SetTrackLocked,
    SetTrackMuted, SetVideoEdits, TrimClips, Unlink,
};
pub use import::import;
pub use model::{
    AudioEdits, Clip, ClipEdits, ClipId, Fit, FreshIds, LinkId, MediaId, MediaInfo, MediaKind,
    MediaRef, Project, Rect, Rotation, Sequence, Track, TrackId, TrackKind, VideoEdits,
};
pub use picture::{ColorMatrix, ColorRange, Picture, PictureLayout};
pub use time::{Frame, MediaTime, Rational};
