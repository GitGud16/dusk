//! Decoding the sample's audio: one second of a 440 Hz tone, mono AAC at 48 kHz.

use std::path::{Path, PathBuf};

use dusk_core::MediaTime;
use dusk_media::{AudioDecoder, MediaError};

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testdata")
        .join(name)
}

/// Reads everything left, as interleaved samples.
fn read_all(decoder: &mut AudioDecoder) -> Vec<f32> {
    let mut all = Vec::new();
    let mut chunk = vec![0.0; 2 * 1000];
    loop {
        let frames = decoder.read(&mut chunk).expect("decoding works");
        if frames == 0 {
            return all;
        }
        all.extend_from_slice(&chunk[..2 * frames]);
    }
}

/// The tone's frequency, from zero crossings of the left channel.
fn frequency(samples: &[f32], rate: f64) -> f64 {
    let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
    let crossings = left
        .windows(2)
        .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
        .count();
    crossings as f64 * rate / left.len() as f64
}

#[test]
fn decodes_to_interleaved_stereo_at_the_requested_rate() {
    let mut decoder = AudioDecoder::open(&testdata("sample-h264-aac.mp4"), 48_000, 2).unwrap();
    let samples = read_all(&mut decoder);
    let frames = samples.len() / 2;
    assert!((47_000..=49_100).contains(&frames), "{frames} frames");
    // Mono becomes the same signal on both channels.
    assert!(samples.chunks(2).all(|pair| pair[0] == pair[1]));
    let middle = &samples[2 * 12_000..2 * 36_000];
    let hz = frequency(middle, 48_000.0);
    assert!((435.0..445.0).contains(&hz), "{hz} Hz");
    let peak = middle.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
    // FFmpeg's sine source peaks at 1/8; spreading mono over two channels scales it by
    // about 0.7.
    assert!(peak > 0.05 && peak <= 1.0, "peak {peak}");
}

#[test]
fn resamples_to_another_rate() {
    let mut decoder = AudioDecoder::open(&testdata("sample-h264-aac.mp4"), 44_100, 2).unwrap();
    let samples = read_all(&mut decoder);
    let frames = samples.len() / 2;
    assert!((43_200..=45_100).contains(&frames), "{frames} frames");
    let hz = frequency(&samples[2 * 11_000..2 * 33_000], 44_100.0);
    assert!((435.0..445.0).contains(&hz), "{hz} Hz");
}

#[test]
fn a_seek_starts_at_the_requested_time() {
    let mut decoder = AudioDecoder::open(&testdata("sample-h264-aac.mp4"), 48_000, 2).unwrap();
    decoder.seek(MediaTime(500_000)).unwrap();
    let frames = read_all(&mut decoder).len() / 2;
    assert!(
        (23_000..=25_100).contains(&frames),
        "{frames} frames after 0.5 s"
    );
    decoder.seek(MediaTime(0)).unwrap();
    let frames = read_all(&mut decoder).len() / 2;
    assert!(
        (47_000..=49_100).contains(&frames),
        "{frames} frames from the start"
    );
}

/// Voice notes and music as phones and messengers make them: Opus in Ogg (WhatsApp and
/// Telegram voice notes), AAC in M4A, AMR-NB, and MP3 with cover art; each a second of a
/// 440 Hz tone.
const MESSENGER_AUDIO: [&str; 4] = ["voice.opus", "voice.m4a", "voice.amr", "song.mp3"];

#[test]
fn voice_notes_and_music_decode() {
    for name in MESSENGER_AUDIO {
        let mut decoder = AudioDecoder::open(&testdata(name), 48_000, 2).unwrap();
        let samples = read_all(&mut decoder);
        let frames = samples.len() / 2;
        // Codec delay and padding may add a little.
        assert!(
            (47_000..=52_000).contains(&frames),
            "{name}: {frames} frames"
        );
        let hz = frequency(&samples[2 * 12_000..2 * 36_000], 48_000.0);
        assert!((430.0..450.0).contains(&hz), "{name}: {hz} Hz");
    }
}

#[test]
fn a_file_without_audio_is_refused() {
    assert!(matches!(
        AudioDecoder::open(&testdata("sample-vp9-10bit.webm"), 48_000, 2),
        Err(MediaError::NoAudio { .. })
    ));
}

#[test]
fn an_audio_decoder_can_move_to_a_worker_thread() {
    fn assert_send<T: Send>() {}
    assert_send::<AudioDecoder>();
}

#[test]
fn a_mono_wav_with_no_channel_order_decodes() {
    // PCM in WAV leaves the channel order unspecified; the decoder must still convert it.
    let chirp = Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/chirp.wav");
    let mut decoder = AudioDecoder::open(&chirp, 48_000, 2).unwrap();
    let mut samples = vec![0.0; 2 * 4_800];
    assert_eq!(decoder.read(&mut samples).unwrap(), 4_800);
    assert!(samples.iter().any(|sample| sample.abs() > 0.1));
    // Mono plays on both channels.
    assert!(samples.chunks(2).all(|frame| frame[0] == frame[1]));
}

#[test]
fn encoder_priming_is_dropped_once() {
    // MP3 and Opus start with priming samples that FFmpeg drops; what is left is exactly the
    // second that was encoded, at the start of media time.
    for (name, rate) in [("song.mp3", 44_100), ("voice.opus", 48_000)] {
        let mut decoder = AudioDecoder::open(&testdata(name), rate, 2).unwrap();
        let frames = read_all(&mut decoder).len() / 2;
        assert_eq!(frames, rate as usize, "{name}");
    }
}
