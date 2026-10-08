//! Mixing the sequence's audio (docs/ARCHITECTURE.md, "Playback"): every enabled clip on an
//! unmuted audio track, decoded, played at its own speed times the playback factor, at its
//! volume and through its fades, summed into interleaved stereo at the rate of the device or
//! the encoder.

use std::sync::Arc;

use dusk_audio::{Envelope, SpeedResampler};
use dusk_core::time::frame_to_media;
use dusk_core::{Clip, ClipEdits, ClipId, Frame, MediaId, MediaTime, Project, Rational, TrackKind};
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
    /// Frames mixed since `start`.
    mixed: u64,
    /// Frames mixed since the mixer was made, across restarts.
    total: u64,
    voices: Vec<Voice>,
    /// Decoders no voice uses now, kept a while so that a clip of the same media starts
    /// without opening the file again, as when playing backwards restarts every clip five
    /// times a second.
    spare: Vec<Spare>,
    /// The gain of each frame of the block being mixed, for one clip; kept to reuse.
    gains: Vec<f32>,
}

/// An open decoder set aside.
struct Spare {
    media: MediaId,
    decoder: AudioDecoder,
    /// `Mixer::total` when it was set aside.
    since: u64,
}

/// Spare decoders unused for this many seconds of mixed sound are closed, as idle decoders
/// are (docs/ARCHITECTURE.md, "Decoder pool").
const SPARE_SECONDS: u64 = 5;
/// At most this many decoders are kept spare; the ones set aside first go first.
const SPARE_LIMIT: usize = 4;

/// A clip being played.
struct Voice {
    clip: ClipId,
    media: MediaId,
    decoder: AudioDecoder,
    resampler: SpeedResampler,
    /// Samples read from the decoder, for the resampler.
    source: Vec<f32>,
    /// Resampled samples not mixed yet.
    ready: Vec<f32>,
    /// The decoder has nothing more; the voice is silent from here.
    ended: bool,
}

/// Mixes a sequence's audio backwards from a timeline time (docs/ARCHITECTURE.md,
/// "Playback": reversed sound from 0.25x to 2x backwards). The sequence is mixed forwards a
/// short stretch at a time, ending where the last stretch began, and each stretch is played
/// from its end.
pub(crate) struct ReverseMixer {
    /// Mixes each stretch forwards.
    mixer: Mixer,
    /// The timeline time the next stretch ends at.
    end: MediaTime,
    /// The current stretch, its frames already in reverse order, and how much of it is out.
    stretch: Vec<f32>,
    given: usize,
}

/// Frames mixed forwards at a time when playing backwards: 0.2 s at 48 kHz.
const STRETCH: usize = 9_600;

impl ReverseMixer {
    /// Mixes `project` backwards from timeline time `start` at `rate` frames per second, the
    /// timeline running back `speed` (positive) seconds per second mixed.
    pub fn new(project: Arc<Project>, rate: u32, start: MediaTime, speed: f64) -> ReverseMixer {
        ReverseMixer {
            mixer: Mixer::new(project, rate, start, speed),
            end: start,
            stretch: Vec::new(),
            given: 0,
        }
    }

    /// Fills `out` with the next interleaved stereo frames.
    pub fn fill(&mut self, out: &mut [f32]) -> Result<(), MediaError> {
        let mut filled = 0;
        while filled < out.len() {
            if self.given == self.stretch.len() {
                self.mix_stretch()?;
            }
            let count = (out.len() - filled).min(self.stretch.len() - self.given);
            out[filled..filled + count]
                .copy_from_slice(&self.stretch[self.given..self.given + count]);
            filled += count;
            self.given += count;
        }
        Ok(())
    }

    /// Mixes the stretch that ends where the last one began, and reverses it.
    fn mix_stretch(&mut self) -> Result<(), MediaError> {
        let span = STRETCH as f64 * 1e6 * self.mixer.factor / f64::from(self.mixer.rate);
        let start = self.end - MediaTime(span.round() as i64);
        self.stretch.resize(2 * STRETCH, 0.0);
        if self.end > MediaTime(0) {
            self.mixer.restart(start);
            self.mixer.fill(&mut self.stretch)?;
        } else {
            // Before the timeline there is nothing to mix.
            self.stretch.fill(0.0);
        }
        // Frame by frame, each frame's two samples staying in order.
        self.stretch.as_chunks_mut::<2>().0.reverse();
        self.given = 0;
        self.end = start;
        Ok(())
    }
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
            total: 0,
            voices: Vec::new(),
            spare: Vec::new(),
            gains: Vec::new(),
        }
    }

    /// Mixes from timeline time `start` from here on, setting the open decoders aside for
    /// the clips that sound there.
    pub fn restart(&mut self, start: MediaTime) {
        for voice in self.voices.drain(..) {
            self.spare.push(Spare {
                media: voice.media,
                decoder: voice.decoder,
                since: self.total,
            });
        }
        self.start = start;
        self.mixed = 0;
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
            let envelope = envelope(clip, rate);
            let mut gains = std::mem::take(&mut self.gains);
            gains.clear();
            if !envelope.is_unity() {
                let into_clip = |frame| (self.time_of(frame) - start).0;
                gains.extend((from..to).map(|frame| envelope.gain_at(into_clip(frame))));
            }
            let scale = (!gains.is_empty()).then_some(gains.as_slice());
            let mixed = self.voices[index].mix(&mut out[range], scale);
            self.gains = gains;
            mixed?;
        }
        self.mixed = last;
        self.total += last - first;
        // A voice whose clip ended, or is gone, starts afresh if it sounds again.
        let ended = |voice: &mut Voice| {
            project
                .find_clip(voice.clip)
                .is_none_or(|(_, clip)| frame_to_media(clip.end(), rate) <= block_end)
        };
        for voice in self.voices.extract_if(.., ended) {
            self.spare.push(Spare {
                media: voice.media,
                decoder: voice.decoder,
                since: self.total,
            });
        }
        let (now, idle) = (self.total, SPARE_SECONDS * u64::from(self.rate));
        self.spare.retain(|spare| now - spare.since < idle);
        let excess = self.spare.len().saturating_sub(SPARE_LIMIT);
        self.spare.drain(..excess);
        Ok(())
    }

    /// Starts playing `clip`, which begins at timeline time `start`, from timeline time `at`,
    /// with a spare decoder of its media if there is one.
    fn start_voice(
        &mut self,
        clip: &Clip,
        start: MediaTime,
        at: MediaTime,
    ) -> Result<Voice, MediaError> {
        let spare = self
            .spare
            .iter()
            .rposition(|spare| spare.media == clip.media_id);
        let mut decoder = match spare {
            Some(index) => self.spare.remove(index).decoder,
            None => {
                let path = self
                    .project
                    .media_ref(clip.media_id)
                    .map(|media| media.path.clone())
                    .unwrap_or_default();
                AudioDecoder::open(&path, self.rate, 2)?
            }
        };
        let into_clip = (at - start).0 as f64 * clip.speed;
        decoder.seek(clip.source_in + MediaTime(into_clip.round() as i64))?;
        Ok(Voice {
            clip: clip.id,
            media: clip.media_id,
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

/// How loud `clip` plays over its length on a timeline at `rate`.
fn envelope(clip: &Clip, rate: Rational) -> Envelope {
    let duration = |frames: Frame| frame_to_media(frames, rate).0;
    let length = frame_to_media(clip.end(), rate).0 - frame_to_media(clip.position, rate).0;
    match &clip.edits {
        ClipEdits::Audio(edits) => Envelope::new(
            edits.volume_db,
            duration(edits.fade_in),
            duration(edits.fade_out),
            length,
        ),
        ClipEdits::Video(_) => Envelope::new(0.0, 0, 0, length),
    }
}

impl Voice {
    /// Adds the voice's next frames to `out`, interleaved stereo, each frame scaled by its
    /// gain in `gains` when there are some.
    fn mix(&mut self, out: &mut [f32], gains: Option<&[f32]>) -> Result<(), MediaError> {
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
        let voice = self.ready.drain(..wanted);
        match gains {
            None => {
                for (sample, voice) in out.iter_mut().zip(voice) {
                    *sample += voice;
                }
            }
            Some(gains) => {
                // Interleaved stereo: both samples of a frame share its gain.
                for ((index, sample), voice) in out.iter_mut().enumerate().zip(voice) {
                    *sample += voice * gains[index / 2];
                }
            }
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
        let audio = project.sequence().tracks().iter();
        let clip = audio
            .filter(|track| track.kind() == dusk_core::TrackKind::Audio)
            .find_map(|track| track.clips().first())
            .unwrap()
            .id;
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

    /// The sample project with its audio clip given `edits`.
    fn with_audio_edits(edits: dusk_core::AudioEdits) -> Project {
        let mut project = project_with_sample_at(Frame(0));
        let audio = audio_clip(&project);
        Command::SetAudioEdits(dusk_core::SetAudioEdits::new(audio, edits))
            .apply(&mut project)
            .unwrap();
        project
    }

    fn audio_clip(project: &Project) -> ClipId {
        project
            .sequence()
            .tracks()
            .iter()
            .filter(|track| track.kind() == TrackKind::Audio)
            .find_map(|track| track.clips().first())
            .unwrap()
            .id
    }

    fn louder(a: &[f32], gain: f32, b: &[f32]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b * gain).abs() < 1e-6)
    }

    #[test]
    fn a_clip_plays_at_its_volume() {
        let edits = dusk_core::AudioEdits {
            volume_db: -6.0,
            ..dusk_core::AudioEdits::default()
        };
        let mixed = mix(with_audio_edits(edits), MediaTime(0), 1.0, 24_000);
        assert!(louder(&mixed, 0.501_187_2, &direct(MediaTime(0), 24_000)));
    }

    #[test]
    fn a_fade_in_rises_from_silence_to_full_volume() {
        // 15 frames at 30 fps: half a second, 24 000 sample frames.
        let edits = dusk_core::AudioEdits {
            fade_in: Frame(15),
            ..dusk_core::AudioEdits::default()
        };
        let mixed = mix(with_audio_edits(edits), MediaTime(0), 1.0, 36_000);
        let source = direct(MediaTime(0), 36_000);
        assert_eq!(mixed[0], 0.0);
        let quiet = mixed[..2 * 1_000].iter().map(|s| s.abs()).sum::<f32>();
        let loud = source[..2 * 1_000].iter().map(|s| s.abs()).sum::<f32>();
        assert!(quiet < loud / 10.0, "{quiet} {loud}");
        // Past the fade it is the source again.
        assert!(louder(&mixed[2 * 24_000..], 1.0, &source[2 * 24_000..]));
    }

    #[test]
    fn disabled_clips_and_muted_tracks_are_silent() {
        let mut project = project_with_sample_at(Frame(0));
        let audio = audio_clip(&project);
        Command::SetClipEnabled(dusk_core::SetClipEnabled::new(audio, false))
            .apply(&mut project)
            .unwrap();
        assert!(silent(&mix(project, MediaTime(0), 1.0, 4_800)));
        let mut project = project_with_sample_at(Frame(0));
        let track = project.find_clip(audio_clip(&project)).unwrap().0.id();
        Command::SetTrackMuted(dusk_core::SetTrackMuted::new(track, true))
            .apply(&mut project)
            .unwrap();
        assert!(silent(&mix(project, MediaTime(0), 1.0, 4_800)));
    }

    /// A one-second chirp from 200 Hz rising, 48 kHz PCM: exact to decode and to seek, and
    /// different backwards.
    fn chirp() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/chirp.wav")
    }

    fn project_with_chirp() -> Project {
        let info = crate::media_info(&chirp()).unwrap();
        let mut project = Project::new(dusk_core::Rational::new(30, 1).unwrap(), (320, 240));
        import(&project, chirp(), info, Frame(0))
            .apply(&mut project)
            .unwrap();
        project
    }

    fn mix_backwards(project: Project, start: MediaTime, speed: f64, frames: usize) -> Vec<f32> {
        let mut mixer = ReverseMixer::new(Arc::new(project), 48_000, start, speed);
        let mut out = vec![1.0; frames * 2];
        for block in out.chunks_mut(2 * 701) {
            mixer.fill(block).unwrap();
        }
        out
    }

    /// `samples` (interleaved stereo) with its frames in reverse order.
    fn reversed(samples: &[f32]) -> Vec<f32> {
        samples.chunks(2).rev().flatten().copied().collect()
    }

    #[test]
    fn playing_backwards_plays_the_sound_reversed() {
        // From 0.5 s back to 0.25 s: the forward mix of 0.25 s to 0.5 s, back to front.
        let forward = mix(project_with_chirp(), MediaTime(250_000), 1.0, 12_000);
        let backward = mix_backwards(project_with_chirp(), MediaTime(500_000), 1.0, 12_000);
        assert!(!silent(&forward));
        let expected = reversed(&forward);
        let worst = backward
            .iter()
            .zip(&expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "differs by up to {worst}");
    }

    #[test]
    fn before_the_start_backwards_there_is_silence() {
        // From 0.1 s backwards: a tenth of a second of sound, then nothing.
        let backward = mix_backwards(project_with_chirp(), MediaTime(100_000), 1.0, 9_600);
        assert!(!silent(&backward[..2 * 4_800]));
        assert!(silent(&backward[2 * 4_800..]));
    }
}
