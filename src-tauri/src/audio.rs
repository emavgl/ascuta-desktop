//! Audio DSP helpers — pure-logic port of Paseo
//! `packages/app/src/hooks/use-dictation-audio-source.web.ts`
//! (`floatToInt16`, `resampleToPcm16`, `concatInt16`, RMS volume)
//! plus the 16kHz-mono contract from `use-dictation.shared.ts`
//! (`audio/pcm;rate=16000;bits=16`, ~1s chunks).
//!
//! Device capture (cpal) is intentionally NOT wired here yet: this module
//! must stay buildable in headless CI with no ALSA/pulse headers. The
//! capture backend lands as `AudioCapture` impl behind the `AudioSource`
//! trait (M1), reusing these helpers verbatim.

pub const TARGET_SAMPLE_RATE: u32 = 16_000;
pub const CHUNK_SAMPLES: usize = 16_000; // ~1s at 16kHz
pub const PCM_FORMAT: &str = "audio/pcm;rate=16000;bits=16";

pub fn float_to_int16(sample: f32) -> i16 {
    let clamped = sample.clamp(-1.0, 1.0);
    if clamped < 0.0 {
        (clamped * 32768.0).round() as i16
    } else {
        (clamped * 32767.0).round() as i16
    }
}

/// Linear-interpolating resample + float→i16, mirroring Paseo
/// `resampleToPcm16`. Input is mono f32 at `input_rate`.
pub fn resample_to_pcm16(input: &[f32], input_rate: u32, output_rate: u32) -> Vec<i16> {
    if input.is_empty() {
        return Vec::new();
    }
    if input_rate == output_rate {
        return input.iter().map(|&s| float_to_int16(s)).collect();
    }
    let ratio = input_rate as f64 / output_rate as f64;
    let out_len = ((input.len() as f64 / ratio).round() as usize).max(1);
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let i0 = src.floor() as usize;
        let i1 = (i0 + 1).min(input.len() - 1);
        let frac = (src - i0 as f64) as f32;
        let s = input[i0] * (1.0 - frac) + input[i1] * frac;
        out.push(float_to_int16(s));
    }
    out
}

/// RMS → 0..1 volume meter, mirroring Paseo `onaudioprocess` meter
/// (`rms * 2`, clamped).
pub fn rms_volume(input: &[f32]) -> f32 {
    if input.is_empty() {
        return 0.0;
    }
    let sum: f32 = input.iter().map(|s| s * s).sum();
    (sum / input.len() as f32).sqrt() * 2.0
}

/// Split PCM16 into `CHUNK_SAMPLES` (~1s) base64 chunks for the STT worker.
/// Matches Paseo `emitPcmSegments`/`DictationStreamSender.enqueueSegment`.
pub fn chunk_pcm16(pcm: &[i16]) -> Vec<Vec<i16>> {
    if pcm.is_empty() {
        return Vec::new();
    }
    pcm.chunks(CHUNK_SAMPLES).map(|c| c.to_vec()).collect()
}

/// Capture backend contract. `CpalCapture` (M1) implements this;
/// `FakeAudioSource` feeds synthetic PCM for `--check`/tests.
pub trait AudioSource {
    fn start(&mut self) -> anyhow::Result<()>;
    fn stop(&mut self) -> anyhow::Result<Vec<i16>>;
    fn volume(&self) -> f32;
}

pub struct FakeAudioSource {
    pub samples: Vec<i16>,
    pub volume: f32,
    pub started: bool,
}

impl FakeAudioSource {
    pub fn with_silence(seconds: u32) -> Self {
        Self {
            samples: vec![0; (TARGET_SAMPLE_RATE * seconds) as usize],
            volume: 0.0,
            started: false,
        }
    }
}

impl AudioSource for FakeAudioSource {
    fn start(&mut self) -> anyhow::Result<()> {
        self.started = true;
        Ok(())
    }
    fn stop(&mut self) -> anyhow::Result<Vec<i16>> {
        self.started = false;
        Ok(std::mem::take(&mut self.samples))
    }
    fn volume(&self) -> f32 {
        self.volume
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_rates_match() {
        let inp = vec![0.0, 0.5, -0.5, 1.0, -1.0];
        let out = resample_to_pcm16(&inp, 16000, 16000);
        assert_eq!(out, vec![0, 16384, -16384, 32767, -32768]);
    }

    #[test]
    fn downsample_halves_length() {
        let inp = vec![0.0f32; 32000];
        let out = resample_to_pcm16(&inp, 32000, 16000);
        assert_eq!(out.len(), 16000);
    }

    #[test]
    fn silence_has_zero_volume() {
        assert_eq!(rms_volume(&[0.0; 128]), 0.0);
    }

    #[test]
    fn chunking_splits_at_1s() {
        let pcm = vec![0i16; CHUNK_SAMPLES * 2 + 100];
        let chunks = chunk_pcm16(&pcm);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[2].len(), 100);
    }

    #[test]
    fn fake_roundtrips() {
        let mut f = FakeAudioSource::with_silence(1);
        f.start().unwrap();
        let pcm = f.stop().unwrap();
        assert_eq!(pcm.len(), 16000);
    }
}
