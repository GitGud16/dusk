//! The Dusk project model: project, sequence, tracks and clips and their edits as plain data,
//! the commands that change them, and their invariants.
//!
//! Depends only on `serde`; no FFmpeg, wgpu, Slint or I/O (see docs/ARCHITECTURE.md).

mod command;
mod model;
pub mod time;

pub use command::{Command, Edge, InsertClips, Notice, Rejection, TrimClips};
pub use model::{
    AudioEdits, Clip, ClipEdits, ClipId, FreshIds, LinkId, MediaId, MediaInfo, MediaKind, MediaRef,
    Project, Sequence, Track, TrackId, TrackKind, VideoEdits,
};
pub use time::{Frame, MediaTime, Rational};
