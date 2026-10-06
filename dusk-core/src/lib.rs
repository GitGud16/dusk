//! The Dusk project model: project, sequence, tracks and clips and their edits as plain data,
//! the commands that change them, and their invariants.
//!
//! Depends only on `serde`; no FFmpeg, wgpu, Slint or I/O (see docs/ARCHITECTURE.md).

pub mod color;
mod command;
pub mod file;
mod import;
mod model;
mod orientation;
mod picture;
mod planar;
mod session;
pub mod time;

pub use command::{
    ApplyClipSession, Command, Edge, InsertClips, MoveClips, Notice, Rejection, RelinkMedia,
    RemoveClips, SEQUENCE_SIDES, SetAudioEdits, SetClipEnabled, SetSequenceSettings,
    SetTrackLocked, SetTrackMuted, SetVideoEdits, SplitClips, TrimClips, Unlink,
    nearest_free_position, remove_one, split_at,
};
pub use import::{STILL_LENGTH, add_media, import, nearest_free_place, place, place_where_free};
pub use model::{
    AudioEdits, Clip, ClipEdits, ClipId, Fit, FreshIds, LinkId, MediaId, MediaInfo, MediaKind,
    MediaRef, Project, Rect, Rotation, Sequence, Track, TrackId, TrackKind, VideoEdits,
};
pub use orientation::Orientation;
pub use picture::{ChromaSiting, ColorMatrix, ColorRange, Picture, PictureLayout, yuv_to_rgb};
pub use planar::{SdrPicture, YuvPicture};
pub use session::{ClipDraft, ClipEditSession, SessionStatus};
pub use time::{Frame, MediaTime, Rational};
