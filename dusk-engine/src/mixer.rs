//! Mixing the sequence's audio (docs/ARCHITECTURE.md, "Playback"): every enabled clip on an
//! unmuted audio track, decoded, played at its own speed times the playback factor, summed
//! into interleaved stereo at the rate of the device or the encoder. Volume and fades arrive
//! in M2.

use std::sync::Arc;

use dusk_audio::SpeedResampler;
use dusk_core::time::frame_to_media;
use dusk_core::{Clip, ClipId, MediaTime, Project, TrackKind};
use dusk_media::{AudioDecoder, MediaError};

/// Source frames read from a decoder at a time.
const CHUNK: usize = 1024;

/// Mixes a sequence's audio forwards from a timeline time.
pub(crate) struct Mixer {
    project: Arc<Project>,
    rate: u32,
    factor: f64,
    /// The timeline time of the first frame mixed.
    start: MediaTime,
    /// Frames mixed so far.
    mixed: u64,
    voices: Vec<Voice>,
}

/// A clip being played.
struct Voice {
    clip: ClipId,
    decoder: AudioDecoder,
    resampler: SpeedResampler,
    /// Samples read from the decoder, for the resampler.
    source: Vec<f32>,
    /// Resampled samples not mixed yet.
    ready: Vec<f32>,
    /// The decoder has nothing more; the voice is silent from here.
    ended: bool,
}

impl Mixer {
    /// Mixes `project` from timeline time `start` at `rate` frames per second, the timeline
    /// advancing `factor` (positive) seconds per second mixed.
    pub fn new(project: Arc<Project>, rate: u32, start: MediaTime, factor: f64) -> Mixer {
        Mixer {
            project,
            rate,
            factor,
            start,
            mixed: 0,
            voices: Vec::new(),
        }
    }

    /// Fills `out` with the next interleaved stereo frames.
    pub fn fill(&mut self, out: &mut [f32]) -> Result<(), MediaError> {
        out.fill(0.0);
        let first = self.mixed;
        let last = first + (out.len() / 2) as u64;
        let block_end = self.time_of(last);
        let project = Arc::clone(&self.project);
        let rate = project.sequence().frame_rate();
        let sounding = project
            .sequence()
            .tracks()
            .iter()
            .filter(|track| track.kind() == TrackKind::Audio && !track.muted())
            .flat_map(|track| track.clips())
            .filter(|clip| clip.enabled);
        for clip in sounding {
            let (start, end) = (
                frame_to_media(clip.position, rate),
                frame_to_media(clip.end(), rate),
            );
            // The frames of this block where the clip sounds.
            let from = self.frame_of(start).clamp(first, last);
            let to = self.frame_of(end).clamp(first, last);
            if from >= to {
                continue;
            }
            let index = match self.voices.iter().position(|voice| voice.clip == clip.id) {
                Some(index) => index,
                None => {
                    let voice = self.start_voice(clip, start, self.time_of(from))?;
                    self.voices.push(voice);
                    self.voices.len() - 1
                }
            };
            let range = 2 * (from - first) as usize..2 * (to - first) as usize;
            self.voices[index].mix(&mut out[range])?;
        }
        // A voice whose clip ended, or is gone, starts afresh if it sounds again.
        self.voices.retain(|voice| {
            project
                .find_clip(voice.clip)
                .is_some_and(|(_, clip)| frame_to_media(clip.end(), rate) > block_end)
        });
        self.mixed = last;
        Ok(())
    }

    /// Starts playing `clip`, which begins at timeline time `start`, from timeline time `at`.
    fn start_voice(
        &self,
        clip: &Clip,
        start: MediaTime,
        at: MediaTime,
    ) -> Result<Voice, MediaError> {
        let path = self
            .project
            .media_ref(clip.media_id)
            .map(|media| media.path.clone())
            .unwrap_or_default();
        let mut decoder = AudioDecoder::open(&path, self.rate, 2)?;
        let into_clip = (at - start).0 as f64 * clip.speed;
        decoder.seek(clip.source_in + MediaTime(into_clip.round() as i64))?;
        Ok(Voice {
            clip: clip.id,
            decoder,
            resampler: SpeedResampler::new(clip.speed * self.factor),
            source: vec![0.0; 2 * CHUNK],
            ready: Vec::new(),
            ended: false,
        })
    }

    /// The timeline time of mixed frame `frame`.
    fn time_of(&self, frame: u64) -> MediaTime {
        let seconds = frame as f64 / f64::from(self.rate) * self.factor;
        self.start + MediaTime((seconds * 1e6).round() as i64)
    }

    /// The first mixed frame at or after timeline time `time`.
    fn frame_of(&self, time: MediaTime) -> u64 {
        let seconds = (time - self.start).0 as f64 / 1e6 / self.factor;
        (seconds * f64::from(self.rate)).ceil().max(0.0) as u64
    }
}

impl Voice {
    /// Adds the voice's next frames to `out`, interleaved stereo.
    fn mix(&mut self, out: &mut [f32]) -> Result<(), MediaError> {
        while self.ready.len() < out.len() {
            if self.ended {
                // Past the end of its stream a clip is silent.
                self.ready.resize(out.len(), 0.0);
                break;
            }
            let frames = self.decoder.read(&mut self.source)?;
            if frames == 0 {
                self.ended = true;
            }
            self.resampler
                .process(&self.source[..2 * frames], &mut self.ready);
        }
        let wanted = out.len();
        for (sample, voice) in out.iter_mut().zip(self.ready.drain(..wanted)) {
            *sample += voice;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use dusk_core::{Command, Edge, Frame, TrimClips, import};

    /// One second of a 440 Hz tone (mono, 48 kHz) with 30 fps video.
    fn sample() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/sample-h264-aac.mp4")
    }

    fn project_with_sample_at(position: Frame) -> Project {
        let info = crate::media_info(&sample()).unwrap();
        let mut project = Project::new(info.frame_rate.unwrap(), (320, 240));
        import(&project, sample(), info, position)
            .apply(&mut project)
            .unwrap();
        project
    }

    fn mix(project: Project, start: MediaTime, factor: f64, frames: usize) -> Vec<f32> {
        let mut mixer = Mixer::new(Arc::new(project), 48_000, start, factor);
        let mut out = vec![1.0; frames * 2];
        // Odd block sizes, so clip edges fall inside blocks.
        for block in out.chunks_mut(2 * 701) {
            mixer.fill(block).unwrap();
        }
        out
    }

    /// The source decoded directly, as stereo, from `from`.
    fn direct(from: MediaTime, frames: usize) -> Vec<f32> {
        let mut decoder = AudioDecoder::open(&sample(), 48_000, 2).unwrap();
        decoder.seek(from).unwrap();
        let mut out = vec![0.0; frames * 2];
        assert_eq!(decoder.read(&mut out).unwrap(), frames);
        out
    }

    fn silent(samples: &[f32]) -> bool {
        samples.iter().all(|sample| *sample == 0.0)
    }

    #[test]
    fn a_clip_plays_its_source_exactly() {
        let mixed = mix(project_with_sample_at(Frame(0)), MediaTime(0), 1.0, 24_000);
        assert_eq!(mixed, direct(MediaTime(0), 24_000));
    }

    #[test]
    fn before_a_clip_starts_there_is_silence() {
        // At frame 15 the clip starts half a second in: 24 000 frames of silence.
        let mixed = mix(project_with_sample_at(Frame(15)), MediaTime(0), 1.0, 30_000);
        assert!(silent(&mixed[..2 * 24_000]));
        assert_eq!(&mixed[2 * 24_000..], &direct(MediaTime(0), 6_000)[..]);
    }

    #[test]
    fn a_trimmed_clip_starts_at_its_new_in_point() {
        let mut project = project_with_sample_at(Frame(0));
        let clip = project.sequence().tracks()[1].clips()[0].id;
        Command::TrimClips(TrimClips::new(clip, Edge::Start, Frame(9)))
            .apply(&mut project)
            .unwrap();
        // The clip now starts at frame 9, 0.3 s into the timeline and into the source.
        let mixed = mix(project, MediaTime(300_000), 1.0, 12_000);
        assert_eq!(mixed, direct(MediaTime(300_000), 12_000));
    }

    #[test]
    fn after_the_last_clip_there_is_silence() {
        let mixed = mix(
            project_with_sample_at(Frame(0)),
            MediaTime(900_000),
            1.0,
            9_600,
        );
        assert!(!silent(&mixed[..2 * 4_800]));
        assert!(silent(&mixed[2 * 4_800..]));
    }

    #[test]
    fn mixing_at_double_speed_covers_twice_the_timeline() {
        // A quarter second of output at 2x covers half a second of the clip.
        let mixed = mix(
            project_with_sample_at(Frame(0)),
            MediaTime(500_000),
            2.0,
            14_400,
        );
        assert!(!silent(&mixed[..2 * 12_000]));
        assert!(silent(&mixed[2 * 12_000 + 2..]));
    }
}
