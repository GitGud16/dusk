//! The media bin's thumbnails (docs/ARCHITECTURE.md, "Memory discipline"): the engine's
//! thumbnail thread makes them, and they are kept here as images ready to draw, within a cap
//! of their own.

use std::collections::{HashMap, HashSet, VecDeque};

use dusk_core::{MediaId, MediaKind, Project};

/// What the thumbnails may take: 64 MB, about 1800 of them.
pub const THUMBNAIL_CAP: usize = 64 * 1024 * 1024;

/// Thumbnails by media, `I` being the image type the window draws.
pub struct Thumbnails<I> {
    cap: usize,
    images: HashMap<MediaId, (I, usize)>,
    /// The order they came in, oldest first, which is the order they go in over the cap.
    order: VecDeque<MediaId>,
    bytes: usize,
    /// Media whose thumbnail was asked for, so that each is asked for once.
    asked: HashSet<MediaId>,
}

impl<I: Clone> Thumbnails<I> {
    pub fn new(cap: usize) -> Thumbnails<I> {
        Thumbnails {
            cap,
            images: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            asked: HashSet::new(),
        }
    }

    /// The videos and photos of `project` whose thumbnail nobody asked for yet, now taken as
    /// asked for. Thumbnails of media no longer in the project are let go.
    pub fn wanted(&mut self, project: &Project) -> Vec<MediaId> {
        let present: HashSet<MediaId> = project.media().iter().map(|media| media.id).collect();
        self.asked.retain(|media| present.contains(media));
        let gone: Vec<MediaId> = self
            .order
            .iter()
            .copied()
            .filter(|media| !present.contains(media))
            .collect();
        for media in gone {
            self.remove(media);
        }
        project
            .media()
            .iter()
            .filter(|media| media.info.kind != MediaKind::Audio)
            .filter_map(|media| self.asked.insert(media.id).then_some(media.id))
            .collect()
    }

    /// Keeps `image`, `bytes` large, as the thumbnail of `media`, letting the oldest go while
    /// they take more than the cap.
    pub fn insert(&mut self, media: MediaId, image: I, bytes: usize) {
        self.remove(media);
        self.images.insert(media, (image, bytes));
        self.order.push_back(media);
        self.bytes += bytes;
        while self.bytes > self.cap {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some((_, bytes)) = self.images.remove(&oldest) {
                self.bytes -= bytes;
            }
        }
    }

    /// Lets the thumbnail of `media` go, so it is asked for again.
    pub fn forget(&mut self, media: MediaId) {
        self.remove(media);
        self.asked.remove(&media);
    }

    fn remove(&mut self, media: MediaId) {
        if let Some((_, bytes)) = self.images.remove(&media) {
            self.bytes -= bytes;
        }
        self.order.retain(|kept| *kept != media);
    }

    /// The thumbnail of `media`, if there is one.
    pub fn get(&self, media: MediaId) -> Option<I> {
        self.images.get(&media).map(|(image, _)| image.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{MediaInfo, MediaTime, Orientation, Rational, add_media};

    fn info(kind: MediaKind) -> MediaInfo {
        let picture = kind != MediaKind::Audio;
        MediaInfo {
            kind,
            duration: MediaTime(1_000_000),
            has_video: picture,
            has_audio: kind != MediaKind::Still,
            frame_rate: None,
            vfr: false,
            width: if picture { 1920 } else { 0 },
            height: if picture { 1080 } else { 0 },
            orientation: Orientation::UPRIGHT,
        }
    }

    /// A project holding a video, a song and a photo, and their ids.
    fn project() -> (Project, [MediaId; 3]) {
        let mut project = Project::new(Rational::new(30, 1).unwrap(), (1920, 1080));
        let ids = [
            ("clip.mp4", MediaKind::Video),
            ("song.mp3", MediaKind::Audio),
            ("photo.jpg", MediaKind::Still),
        ]
        .map(|(name, kind)| {
            let (id, mut command) = add_media(&project, name.into(), info(kind));
            command.apply(&mut project).unwrap();
            id
        });
        (project, ids)
    }

    #[test]
    fn each_video_and_photo_is_asked_for_once() {
        let (project, [video, _, photo]) = project();
        let mut thumbnails = Thumbnails::<u8>::new(100);
        assert_eq!(thumbnails.wanted(&project), [video, photo]);
        assert_eq!(thumbnails.wanted(&project), []);
    }

    #[test]
    fn the_oldest_go_once_over_the_cap() {
        let (project, [video, _, photo]) = project();
        let mut thumbnails = Thumbnails::new(100);
        thumbnails.wanted(&project);
        thumbnails.insert(video, 'v', 60);
        assert_eq!(thumbnails.get(video), Some('v'));
        thumbnails.insert(photo, 'p', 60);
        assert_eq!(thumbnails.get(video), None);
        assert_eq!(thumbnails.get(photo), Some('p'));
        // Gone for good: asking again would only push others out.
        assert_eq!(thumbnails.wanted(&project), []);
    }

    #[test]
    fn media_gone_from_the_project_take_their_thumbnails_with_them() {
        let (project, [video, _, photo]) = project();
        let mut thumbnails = Thumbnails::new(100);
        thumbnails.wanted(&project);
        thumbnails.insert(video, 'v', 10);
        thumbnails.insert(photo, 'p', 10);
        let empty = Project::new(Rational::new(30, 1).unwrap(), (1920, 1080));
        assert_eq!(thumbnails.wanted(&empty), []);
        assert_eq!(thumbnails.get(video), None);
        // Brought back, by undo for example, they are asked for again.
        assert_eq!(thumbnails.wanted(&project), [video, photo]);
    }

    #[test]
    fn a_forgotten_thumbnail_is_asked_for_again() {
        // A media file found somewhere else: the thumbnail of what was there, or the lack of
        // one, no longer counts.
        let (project, [video, _, photo]) = project();
        let mut thumbnails = Thumbnails::new(100);
        thumbnails.wanted(&project);
        thumbnails.insert(video, 'v', 10);
        thumbnails.forget(video);
        thumbnails.forget(photo);
        assert_eq!(thumbnails.get(video), None);
        assert_eq!(thumbnails.wanted(&project), [video, photo]);
    }
}
