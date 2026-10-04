//! The audio output device (docs/ARCHITECTURE.md, "Threading model"). Its callback runs on a
//! real-time thread, so it only copies from a lock-free ring buffer the engine fills and counts
//! what it played: no allocation, no locks, no waiting.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::FrameCounter;

/// Why the audio output could not be used.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// The default output device could not be opened.
    #[error(
        "the audio device could not be opened ({0}); check the sound settings, or play without \
         sound"
    )]
    Open(String),
    /// The device refused to start or stop.
    #[error("the audio device did not respond ({0}); check the sound settings")]
    Device(String),
}

/// The default output device, fed with interleaved stereo `f32` frames.
pub struct AudioOutput {
    stream: cpal::Stream,
    producer: rtrb::Producer<f32>,
    shared: Arc<Shared>,
}

/// What the device callback shares with the rest of the engine.
struct Shared {
    rate: u32,
    epoch: Instant,
    /// Frames taken from the ring buffer by the device so far.
    played: AtomicU64,
    /// `played` before the last callback, when it ran (microseconds since `epoch`) and the
    /// latency it reported (microseconds until its first frame is heard). Written by the
    /// callback under `sequence`, which is odd while a write is under way.
    sequence: AtomicU64,
    played_before: AtomicU64,
    callback_us: AtomicU64,
    latency_us: AtomicU64,
    /// Set to drop everything queued; the callback empties the buffer and clears it.
    flush: AtomicBool,
}

impl FrameCounter for Shared {
    fn frames_heard(&self) -> u64 {
        loop {
            let sequence = self.sequence.load(Ordering::Acquire);
            if sequence % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let before = self.played_before.load(Ordering::Relaxed);
            let callback = self.callback_us.load(Ordering::Relaxed);
            let latency = self.latency_us.load(Ordering::Relaxed);
            let played = self.played.load(Ordering::Relaxed);
            if self.sequence.load(Ordering::Acquire) != sequence {
                continue;
            }
            // The last callback's first frame is heard `latency` after it ran; frames follow
            // at the device rate from there, never beyond what the device was given.
            let now = self.epoch.elapsed().as_micros() as i128;
            let since = now - i128::from(callback) - i128::from(latency);
            let heard = i128::from(before) + since * i128::from(self.rate) / 1_000_000;
            return heard.clamp(0, i128::from(played)) as u64;
        }
    }

    fn rate(&self) -> u32 {
        self.rate
    }
}

impl AudioOutput {
    /// Opens the default output device, stopped. `Ok(None)` when the machine has no output
    /// device; playback then runs on the system clock without sound.
    pub fn open() -> Result<Option<AudioOutput>, AudioError> {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            return Ok(None);
        };
        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::Open(e.to_string()))?;
        let config = supported.config();
        let rate = config.sample_rate;
        // A fifth of a second of stereo frames: the engine refills it long before it runs dry.
        let (producer, consumer) = rtrb::RingBuffer::new(rate as usize / 5 * 2);
        let shared = Arc::new(Shared {
            rate,
            epoch: Instant::now(),
            played: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            played_before: AtomicU64::new(0),
            callback_us: AtomicU64::new(0),
            latency_us: AtomicU64::new(0),
            flush: AtomicBool::new(false),
        });
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, config, consumer, &shared),
            cpal::SampleFormat::I16 => build::<i16>(&device, config, consumer, &shared),
            cpal::SampleFormat::U16 => build::<u16>(&device, config, consumer, &shared),
            other => Err(format!("the device wants {other:?} samples")),
        }
        .map_err(AudioError::Open)?;
        stream
            .pause()
            .map_err(|e| AudioError::Device(e.to_string()))?;
        Ok(Some(AudioOutput {
            stream,
            producer,
            shared,
        }))
    }

    /// Frames per second of the device.
    pub fn rate(&self) -> u32 {
        self.shared.rate
    }

    /// What the playback clock counts.
    pub fn frame_counter(&self) -> Arc<dyn FrameCounter> {
        self.shared.clone()
    }

    /// How many stereo frames fit in the buffer now.
    pub fn space(&self) -> usize {
        self.producer.slots() / 2
    }

    /// Queues interleaved stereo frames and returns how many it took; the rest did not fit.
    pub fn write(&mut self, stereo: &[f32]) -> usize {
        let frames = (stereo.len() / 2).min(self.space());
        if let Ok(chunk) = self.producer.write_chunk_uninit(frames * 2) {
            chunk.fill_from_iter(stereo[..frames * 2].iter().copied());
        }
        frames
    }

    /// Starts the device playing what is queued.
    pub fn start(&self) -> Result<(), AudioError> {
        self.stream
            .play()
            .map_err(|e| AudioError::Device(e.to_string()))
    }

    /// Drops what is still queued and stops the device.
    pub fn stop(&self) -> Result<(), AudioError> {
        self.shared.flush.store(true, Ordering::Release);
        // Only the callback may empty the buffer, so let it run until it has.
        self.start()?;
        let deadline = Instant::now() + Duration::from_millis(250);
        while self.shared.flush.load(Ordering::Acquire) && Instant::now() < deadline {
            sleep(Duration::from_millis(2));
        }
        self.stream
            .pause()
            .map_err(|e| AudioError::Device(e.to_string()))
    }
}

/// Builds the device stream with samples of type `T`.
fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut consumer: rtrb::Consumer<f32>,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = usize::from(config.channels).max(1);
    let shared = Arc::clone(shared);
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let timestamp = info.timestamp();
                let latency = timestamp
                    .playback
                    .checked_duration_since(timestamp.callback)
                    .unwrap_or_default();
                if shared.flush.load(Ordering::Acquire) {
                    if let Ok(chunk) = consumer.read_chunk(consumer.slots()) {
                        chunk.commit_all();
                    }
                    shared.flush.store(false, Ordering::Release);
                }
                let taken = fill(data, channels, &mut consumer);
                let before = shared.played.load(Ordering::Relaxed);
                shared.sequence.fetch_add(1, Ordering::AcqRel);
                shared.played_before.store(before, Ordering::Relaxed);
                shared
                    .callback_us
                    .store(shared.epoch.elapsed().as_micros() as u64, Ordering::Relaxed);
                shared
                    .latency_us
                    .store(latency.as_micros() as u64, Ordering::Relaxed);
                shared
                    .played
                    .store(before + taken as u64, Ordering::Relaxed);
                shared.sequence.fetch_add(1, Ordering::AcqRel);
            },
            // Errors show up as a clock that stops; the engine notices and reports it.
            |_error| {},
            None,
        )
        .map_err(|e| e.to_string())
}

/// Copies stereo frames from `ring` into `data`, which has `channels` interleaved channels:
/// left and right go to the first two (both averaged into one for a mono device), others get
/// silence, and so does every frame the ring cannot supply. Returns the frames taken.
fn fill<T>(data: &mut [T], channels: usize, ring: &mut rtrb::Consumer<f32>) -> usize
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let frames = data.len() / channels;
    let taken = frames.min(ring.slots() / 2);
    let mut samples = match ring.read_chunk(taken * 2) {
        Ok(chunk) => Some(chunk.into_iter()),
        Err(_) => None,
    };
    for frame in data.chunks_mut(channels) {
        let pair = samples
            .as_mut()
            .and_then(|samples| Some((samples.next()?, samples.next()?)));
        let (left, right) = pair.unwrap_or((0.0, 0.0));
        for (channel, sample) in frame.iter_mut().enumerate() {
            let value = match (channels, channel) {
                (1, _) => (left + right) / 2.0,
                (_, 0) => left,
                (_, 1) => right,
                _ => 0.0,
            };
            *sample = T::from_sample(value);
        }
    }
    taken
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(stereo: &[f32]) -> rtrb::Consumer<f32> {
        let (mut producer, consumer) = rtrb::RingBuffer::new(64);
        for sample in stereo {
            producer.push(*sample).unwrap();
        }
        consumer
    }

    #[test]
    fn stereo_fills_the_first_two_channels_and_silence_the_rest() {
        let mut consumer = ring(&[0.1, 0.2, 0.3, 0.4]);
        let mut data = [9.0f32; 4 * 3];
        assert_eq!(fill(&mut data, 4, &mut consumer), 2);
        assert_eq!(
            data,
            [0.1, 0.2, 0.0, 0.0, 0.3, 0.4, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn a_mono_device_gets_the_average() {
        let mut consumer = ring(&[0.2, 0.4]);
        let mut data = [9.0f32; 1];
        assert_eq!(fill(&mut data, 1, &mut consumer), 1);
        assert!((data[0] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn integer_devices_get_converted_samples() {
        let mut consumer = ring(&[1.0, -1.0]);
        let mut data = [0i16; 2];
        fill(&mut data, 2, &mut consumer);
        assert_eq!(data, [i16::MAX, i16::MIN]);
    }

    /// Plays a tenth of a second of silence on the default device, if there is one.
    #[test]
    fn the_device_counts_what_it_plays() {
        let Some(mut output) = AudioOutput::open().expect("the device opens") else {
            eprintln!("no audio output device here; nothing to check");
            return;
        };
        let rate = output.rate() as usize;
        assert_eq!(output.write(&vec![0.0; rate / 10 * 2]), rate / 10);
        let counter = output.frame_counter();
        output.start().unwrap();
        sleep(Duration::from_millis(400));
        let heard = counter.frames_heard() as usize;
        assert!(
            heard > rate / 20 && heard <= rate / 10,
            "{heard} frames heard"
        );
        output.stop().unwrap();
        assert_eq!(output.space(), rate / 5);
    }
}
