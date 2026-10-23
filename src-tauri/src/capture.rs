//! Microphone capture via CPAL (feature `capture`). Implements
//! `audio::AudioSource`: mono-downmix in the device callback (no inference,
//! no allocation beyond the shared buffer), resample to 16 kHz on `stop`
//! with the same helpers as the Paseo port in `audio.rs`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream};

use crate::audio::{resample_to_pcm16, rms_volume, AudioSource};

/// Hard cap so a stuck hotkey cannot grow the buffer without bound.
const MAX_SECONDS: usize = 120;

pub struct CpalCapture {
    stream: Option<Stream>,
    buffer: Arc<Mutex<Vec<f32>>>,
    volume: Arc<AtomicU32>,
    rate: u32,
}

impl CpalCapture {
    pub fn new() -> Result<Self> {
        Ok(Self {
            stream: None,
            buffer: Arc::new(Mutex::new(Vec::new())),
            volume: Arc::new(AtomicU32::new(0)),
            rate: 16_000,
        })
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    buffer: Arc<Mutex<Vec<f32>>>,
    volume: Arc<AtomicU32>,
    cap: usize,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mono: Vec<f32> = data
                .chunks(channels.max(1))
                .map(|f| f.iter().map(|&s| f32::from_sample_(s)).sum::<f32>() / f.len() as f32)
                .collect();
            volume.store(rms_volume(&mono).min(1.0).to_bits(), Ordering::Relaxed);
            if let Ok(mut b) = buffer.lock() {
                if b.len() + mono.len() <= cap {
                    b.extend_from_slice(&mono);
                }
            }
        },
        |e| eprintln!("ascuta: audio stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

impl AudioSource for CpalCapture {
    fn start(&mut self) -> Result<()> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("no input device"))?;
        let supported = device
            .default_input_config()
            .context("default input config")?;
        self.rate = supported.sample_rate().0;
        let channels = supported.channels() as usize;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        self.buffer.lock().unwrap().clear();
        self.volume.store(0, Ordering::Relaxed);
        let cap = self.rate as usize * MAX_SECONDS;
        let (b, v) = (self.buffer.clone(), self.volume.clone());
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, &config, channels, b, v, cap)?,
            SampleFormat::I16 => build::<i16>(&device, &config, channels, b, v, cap)?,
            SampleFormat::U16 => build::<u16>(&device, &config, channels, b, v, cap)?,
            other => return Err(anyhow!("unsupported sample format {other:?}")),
        };
        stream.play()?;
        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) -> Result<Vec<i16>> {
        self.stream.take(); // dropping the stream stops capture
        let samples = std::mem::take(&mut *self.buffer.lock().unwrap());
        Ok(resample_to_pcm16(&samples, self.rate, 16_000))
    }

    fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }
}
