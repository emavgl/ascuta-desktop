//! Voice-activity trim for the push-to-talk burst (PLAN §7 step 3).
//!
//! Pure-Rust energy VAD on 16 kHz PCM16: frames of 20 ms, adaptive noise
//! floor taken from the quietest frames, so a noisy room does not count as
//! speech. Returns the speech span plus padding, or `None` for a no-speech
//! burst (the caller aborts without pasting). The Silero model from the
//! plan can replace `detect_span` behind the same signature.

use crate::audio::TARGET_SAMPLE_RATE;

const FRAME: usize = (TARGET_SAMPLE_RATE as usize) / 50; // 20 ms
/// Minimum voiced time to count as speech.
const MIN_SPEECH_FRAMES: usize = 5; // 100 ms
/// Padding kept around the detected speech (also acts as pre/post-roll).
const PAD_FRAMES: usize = 12; // 240 ms
/// Absolute floor (RMS, 0..1) so digital silence never triggers.
const MIN_THRESHOLD: f32 = 0.008;
/// Cap on the adaptive threshold so a burst with no leading silence (the
/// quietest frames are already speech) is still detected.
const MAX_THRESHOLD: f32 = 0.02;

fn frame_rms(frame: &[i16]) -> f32 {
    let sum: f64 = frame
        .iter()
        .map(|&s| {
            let v = s as f64 / 32768.0;
            v * v
        })
        .sum();
    (sum / frame.len() as f64).sqrt() as f32
}

/// Index range `[start, end)` (in samples) containing speech, if any.
pub fn detect_span(pcm: &[i16]) -> Option<(usize, usize)> {
    let energies: Vec<f32> = pcm.chunks(FRAME).map(frame_rms).collect();
    if energies.len() < MIN_SPEECH_FRAMES {
        return None;
    }
    let mut sorted = energies.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let noise_floor = sorted[sorted.len() / 10];
    let threshold = (noise_floor * 3.0).clamp(MIN_THRESHOLD, MAX_THRESHOLD);

    let voiced: Vec<usize> = energies
        .iter()
        .enumerate()
        .filter(|(_, &e)| e > threshold)
        .map(|(i, _)| i)
        .collect();
    if voiced.len() < MIN_SPEECH_FRAMES {
        return None;
    }
    let first = voiced[0].saturating_sub(PAD_FRAMES);
    let last = (voiced[voiced.len() - 1] + 1 + PAD_FRAMES).min(energies.len());
    Some((first * FRAME, (last * FRAME).min(pcm.len())))
}

/// Trim leading/trailing silence; `None` when there is no speech.
pub fn trim(pcm: &[i16]) -> Option<&[i16]> {
    detect_span(pcm).map(|(s, e)| &pcm[s..e])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(samples: usize, amp: f32) -> Vec<i16> {
        (0..samples)
            .map(|i| ((i as f32 * 0.2).sin() * amp * 32767.0) as i16)
            .collect()
    }

    #[test]
    fn silence_is_no_speech() {
        assert!(trim(&vec![0i16; 16_000]).is_none());
    }

    #[test]
    fn constant_low_noise_is_no_speech() {
        assert!(trim(&tone(16_000, 0.003)).is_none());
    }

    #[test]
    fn trims_around_speech() {
        let mut pcm = vec![0i16; 16_000];
        pcm.extend(tone(8_000, 0.3));
        pcm.extend(vec![0i16; 16_000]);
        let out = trim(&pcm).expect("speech");
        assert!(out.len() < pcm.len());
        assert!(out.len() >= 8_000);
        // padded, but well inside the original silence
        assert!(out.len() <= 8_000 + 2 * PAD_FRAMES * FRAME + FRAME);
    }

    #[test]
    fn continuous_speech_without_silence() {
        assert!(trim(&tone(8_000, 0.3)).is_some());
    }

    #[test]
    fn tiny_clicks_ignored() {
        let mut pcm = vec![0i16; 16_000];
        pcm[8_000..8_100].fill(20_000);
        assert!(trim(&pcm).is_none());
    }

    #[test]
    fn too_short_input() {
        assert!(trim(&[1000; 10]).is_none());
    }
}
