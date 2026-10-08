//! The frame cache (docs/ARCHITECTURE.md, "Memory discipline"): every decoded frame in the
//! app lives here and nowhere else, in buffers the cache owns, under a byte cap. The least
//! recently used frames go first.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use dusk_core::{MediaId, MediaTime, Picture};
use dusk_media::Following;

/// The default cap, a user setting later (docs/REQUIREMENTS.md).
pub(crate) const DEFAULT_CAP: usize = 384 * 1024 * 1024;

/// Decoded frames by media file and start time. Video frames are kept at their source size,
/// so the start time identifies them; stills, cached at the size they are drawn at, will add
/// the size to the key.
pub(crate) struct FrameCache {
    cap: usize,
    used: usize,
    /// Counts lookups and inserts, to tell which frame was used least recently.
    clock: u64,
    media: HashMap<MediaId, BTreeMap<MediaTime, Entry>>,
}

struct Entry {
    picture: Arc<Picture>,
    /// What follows the frame; it is shown until the next frame starts.
    following: Following,
    last_used: u64,
}

impl FrameCache {
    /// An empty cache that holds at most `cap` bytes of pictures.
    pub fn new(cap: usize) -> FrameCache {
        FrameCache {
            cap,
            used: 0,
            clock: 0,
            media: HashMap::new(),
        }
    }

    /// The picture of `media` shown at `time`, if it is cached and known to cover `time`.
    /// The caller shares it only while drawing it.
    pub fn get(&mut self, media: MediaId, time: MediaTime) -> Option<Arc<Picture>> {
        self.clock += 1;
        let (start, entry) = self.media.get_mut(&media)?.range_mut(..=time).next_back()?;
        let covers = match entry.following {
            Following::Next(next) => time < next,
            Following::End => true,
            Following::Unknown => time == *start,
        };
        covers.then(|| {
            entry.last_used = self.clock;
            Arc::clone(&entry.picture)
        })
    }

    /// The cached picture of `media` whose start is nearest to `time`, for scrubbing.
    pub fn nearest(&mut self, media: MediaId, time: MediaTime) -> Option<Arc<Picture>> {
        self.clock += 1;
        let frames = self.media.get_mut(&media)?;
        let before = frames.range(..=time).next_back().map(|(start, _)| *start);
        let after = frames.range(time..).next().map(|(start, _)| *start);
        let start = match (before, after) {
            (Some(before), Some(after)) if after.0 - time.0 < time.0 - before.0 => after,
            (Some(before), _) => before,
            (None, after) => after?,
        };
        let entry = frames.get_mut(&start)?;
        entry.last_used = self.clock;
        Some(Arc::clone(&entry.picture))
    }

    /// The most bytes of pictures it holds.
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Keeps the frame of `media` that starts at `time`, followed by `following`, and makes
    /// room for it by dropping the least recently used frames.
    pub fn insert(
        &mut self,
        media: MediaId,
        time: MediaTime,
        following: Following,
        picture: Picture,
    ) -> Arc<Picture> {
        self.clock += 1;
        if let Some(old) = self.media.get_mut(&media).and_then(|f| f.remove(&time)) {
            self.used -= old.picture.byte_size();
        }
        let size = picture.byte_size();
        while self.used + size > self.cap && self.evict_one() {}
        let picture = Arc::new(picture);
        self.used += size;
        self.media.entry(media).or_default().insert(
            time,
            Entry {
                picture: Arc::clone(&picture),
                following,
                last_used: self.clock,
            },
        );
        picture
    }

    /// Bytes of pictures held.
    #[cfg(test)]
    fn used(&self) -> usize {
        self.used
    }

    /// Drops the least recently used frame; false when the cache is empty.
    fn evict_one(&mut self) -> bool {
        let oldest = self
            .media
            .iter()
            .flat_map(|(media, frames)| {
                frames
                    .iter()
                    .map(move |(time, entry)| (entry.last_used, *media, *time))
            })
            .min();
        let Some((_, media, time)) = oldest else {
            return false;
        };
        let frames = self.media.get_mut(&media);
        if let Some(entry) = frames.and_then(|frames| frames.remove(&time)) {
            self.used -= entry.picture.byte_size();
        }
        self.media.retain(|_, frames| !frames.is_empty());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{ColorMatrix, ColorRange, PictureLayout};

    /// A 16x16 8-bit picture: 384 bytes.
    fn picture(shade: u8) -> Picture {
        Picture {
            width: 16,
            height: 16,
            layout: PictureLayout::Nv12,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            primaries: dusk_core::color::Primaries::Bt709,
            transfer: dusk_core::color::Transfer::Bt1886,
            peak_nits: 0,
            luma: vec![shade; 256],
            chroma: vec![128; 128],
        }
    }

    const CLIP: MediaId = MediaId(1);
    const OTHER: MediaId = MediaId(2);

    fn us(micros: i64) -> MediaTime {
        MediaTime(micros)
    }

    fn shade(picture: Option<Arc<Picture>>) -> Option<u8> {
        picture.map(|picture| picture.luma[0])
    }

    #[test]
    fn a_frame_covers_the_time_until_the_next_one_starts() {
        let mut cache = FrameCache::new(DEFAULT_CAP);
        cache.insert(CLIP, us(1000), Following::Next(us(2000)), picture(1));
        assert_eq!(shade(cache.get(CLIP, us(1000))), Some(1));
        assert_eq!(shade(cache.get(CLIP, us(1999))), Some(1));
        assert!(cache.get(CLIP, us(2000)).is_none());
        assert!(cache.get(CLIP, us(999)).is_none());
        assert!(cache.get(OTHER, us(1500)).is_none());
    }

    #[test]
    fn the_last_frame_covers_everything_after_it() {
        let mut cache = FrameCache::new(DEFAULT_CAP);
        cache.insert(CLIP, us(1000), Following::End, picture(1));
        assert_eq!(shade(cache.get(CLIP, us(9_000_000))), Some(1));
    }

    #[test]
    fn a_frame_with_an_unknown_successor_covers_only_its_start() {
        let mut cache = FrameCache::new(DEFAULT_CAP);
        cache.insert(CLIP, us(1000), Following::Unknown, picture(1));
        assert_eq!(shade(cache.get(CLIP, us(1000))), Some(1));
        assert!(cache.get(CLIP, us(1001)).is_none());
    }

    #[test]
    fn the_nearest_frame_is_found_on_either_side() {
        let mut cache = FrameCache::new(DEFAULT_CAP);
        cache.insert(CLIP, us(1000), Following::Next(us(2000)), picture(1));
        cache.insert(CLIP, us(5000), Following::Next(us(6000)), picture(5));
        assert_eq!(shade(cache.nearest(CLIP, us(2900))), Some(1));
        assert_eq!(shade(cache.nearest(CLIP, us(3100))), Some(5));
        assert_eq!(shade(cache.nearest(CLIP, us(0))), Some(1));
        assert_eq!(shade(cache.nearest(CLIP, us(99_999))), Some(5));
        assert!(cache.nearest(OTHER, us(1000)).is_none());
    }

    #[test]
    fn the_least_recently_used_frames_make_room() {
        // Room for three pictures.
        let size = picture(0).byte_size();
        let mut cache = FrameCache::new(3 * size);
        for n in 0..3 {
            cache.insert(
                CLIP,
                us(n * 1000),
                Following::Next(us(n * 1000 + 1000)),
                picture(n as u8),
            );
        }
        assert_eq!(cache.used(), 3 * size);
        // Frame 0 is used again, so frame 1 is now the least recently used.
        assert!(cache.get(CLIP, us(0)).is_some());
        cache.insert(OTHER, us(0), Following::End, picture(9));
        assert_eq!(cache.used(), 3 * size);
        assert!(cache.get(CLIP, us(1000)).is_none());
        assert!(cache.get(CLIP, us(0)).is_some());
        assert!(cache.get(CLIP, us(2000)).is_some());
        assert!(cache.get(OTHER, us(0)).is_some());
    }

    #[test]
    fn inserting_a_frame_again_replaces_it() {
        let size = picture(0).byte_size();
        let mut cache = FrameCache::new(DEFAULT_CAP);
        cache.insert(CLIP, us(0), Following::Unknown, picture(1));
        cache.insert(CLIP, us(0), Following::Next(us(1000)), picture(2));
        assert_eq!(cache.used(), size);
        assert_eq!(shade(cache.get(CLIP, us(999))), Some(2));
    }
}
