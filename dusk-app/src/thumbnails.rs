//! The media bin's thumbnails (docs/ARCHITECTURE.md, "Memory discipline"): the engine's
//! thumbnail thread makes them, and they are kept here as images ready to draw, within a cap
//! of their own.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use dusk_core::{MediaId, MediaKind, Project};

/// What the thumbnails may take: 64 MB, about 1800 of them.
pub const THUMBNAIL_CAP: usize = 64 * 1024 * 1024;

/// Thumbnails by media, `I` being the image type the window draws. Each belongs to the file it
/// was made from, so a media found in another file (relinked, or an id given to new media)
/// gets a new one.
pub struct Thumbnails<I> {
    cap: usize,
    images: HashMap<MediaId, (PathBuf, I, usize)>,
    /// The order they came in, oldest first, which is the order they go in over the cap.
    order: VecDeque<MediaId>,
    bytes: usize,
    /// Media whose thumbnail was asked for, and of which file, so that each is asked for once.
    asked: HashMap<MediaId, PathBuf>,
}

impl<I: Clone> Thumbnails<I> {
    pub fn new(cap: usize) -> Thumbnails<I> {
        Thumbnails {
            cap,
            images: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            asked: HashMap::new(),
        }
    }

    /// The videos and photos of `project` whose thumbnail nobody asked for yet, now taken as
    /// asked for. Thumbnails of media no longer in the project, or of a file it no longer
    /// names for them, are let go.
    pub fn wanted(&mut self, project: &Project) -> Vec<MediaId> {
        let files: HashMap<MediaId, &Path> = project
            .media()
            .iter()
            .map(|media| (media.id, media.path.as_path()))
            .collect();
        let current = |media: &MediaId, path: &PathBuf| files.get(media) == Some(&path.as_path());
        self.asked.retain(|media, path| current(media, path));
        let gone: Vec<MediaId> = self
            .images
            .iter()
            .filter(|(media, (path, _, _))| !current(media, path))
            .map(|(media, _)| *media)
            .collect();
        for media in gone {
            self.remove(media);
        }
        let mut wanted = Vec::new();
        for media in project.media() {
            if media.info.kind != MediaKind::Audio && !self.asked.contains_key(&media.id) {
                self.asked.insert(media.id, media.path.clone());
                wanted.push(media.id);
            }
        }
        wanted
    }

    /// Keeps `image`, `bytes` large, as the thumbnail of `media` made from the file at `path`,
    /// letting the oldest go while they take more than the cap. One made from a file other
    /// than the one asked for, which the media no longer names, is not kept.
    pub fn insert(&mut self, media: MediaId, path: PathBuf, image: I, bytes: usize) {
        if self.asked.get(&media) != Some(&path) {
            return;
        }
        self.remove(media);
        self.images.insert(media, (path, image, bytes));
        self.order.push_back(media);
        self.bytes += bytes;
        while self.bytes > self.cap {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some((_, _, bytes)) = self.images.remove(&oldest) {
                self.bytes -= bytes;
            }
        }
    }

    fn remove(&mut self, media: MediaId) {
        if let Some((_, _, bytes)) = self.images.remove(&media) {
            self.bytes -= bytes;
        }
        self.order.retain(|kept| *kept != media);
    }

    /// The thumbnail of `media`, if there is one.
    pub fn get(&self, media: MediaId) -> Option<I> {
        self.images.get(&media).map(|(_, image, _)| image.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dusk_core::{Command, MediaInfo, MediaTime, Orientation, Rational, RelinkMedia, add_media};
    use std::path::Path;

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
        thumbnails.insert(video, "clip.mp4".into(), 'v', 60);
        assert_eq!(thumbnails.get(video), Some('v'));
        thumbnails.insert(photo, "photo.jpg".into(), 'p', 60);
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
        thumbnails.insert(video, "clip.mp4".into(), 'v', 10);
        thumbnails.insert(photo, "photo.jpg".into(), 'p', 10);
        let empty = Project::new(Rational::new(30, 1).unwrap(), (1920, 1080));
        assert_eq!(thumbnails.wanted(&empty), []);
        assert_eq!(thumbnails.get(video), None);
        // Brought back, by undo for example, they are asked for again.
        assert_eq!(thumbnails.wanted(&project), [video, photo]);
    }

    #[test]
    fn media_found_in_another_file_have_their_thumbnails_asked_for_again() {
        // A relinked file, or an id given to new media: the thumbnail of the file it named
        // before no longer counts.
        let (project, [video, _, photo]) = project();
        let mut thumbnails = Thumbnails::new(100);
        thumbnails.wanted(&project);
        thumbnails.insert(video, "clip.mp4".into(), 'v', 10);
        let moved = elsewhere(&project, &[video, photo]);
        assert_eq!(thumbnails.wanted(&moved), [video, photo]);
        assert_eq!(thumbnails.get(video), None);
    }

    #[test]
    fn a_thumbnail_of_a_file_the_media_no_longer_names_is_not_kept() {
        // Made from the file before a relink, it arrives after it.
        let (project, [video, _, _]) = project();
        let mut thumbnails = Thumbnails::new(100);
        let moved = elsewhere(&project, &[video]);
        thumbnails.wanted(&moved);
        thumbnails.insert(video, "clip.mp4".into(), 'v', 10);
        assert_eq!(thumbnails.get(video), None);
        assert_eq!(thumbnails.wanted(&moved), []);
    }

    /// `project` with `media` found in a folder of their own.
    fn elsewhere(project: &Project, media: &[MediaId]) -> Project {
        let mut moved = project.clone();
        for &id in media {
            let found = moved.media_ref(id).unwrap().clone();
            let path = Path::new("found").join(&found.path);
            Command::RelinkMedia(RelinkMedia::new(id, path, found.info))
                .apply(&mut moved)
                .unwrap();
        }
        moved
    }
}
