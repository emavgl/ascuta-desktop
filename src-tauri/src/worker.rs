//! Dedicated dictation worker thread (PLAN §4: "UI never runs inference").
//!
//! Port of the Paseo `worker-process.ts` / `STTManager` pipeline in-process:
//! jobs (PCM16 @16 kHz) go in over a channel, tidy transcripts come out via a
//! callback. The model is loaded on the worker thread so startup is never
//! blocked by a 600 MB ONNX load.

use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use anyhow::Result;

/// Anything that turns 16 kHz mono f32 samples into text.
pub trait Transcriber {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String>;
}

/// Deterministic backend for tests / `--check`.
pub struct FakeTranscriber {
    pub reply: String,
}

impl Transcriber for FakeTranscriber {
    fn transcribe(&mut self, _samples: &[f32]) -> Result<String> {
        Ok(self.reply.clone())
    }
}

pub struct Worker {
    tx: Option<Sender<Vec<i16>>>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    /// `factory` runs on the worker thread. `on_result` receives the final
    /// (tidied) text; an empty string means "nothing to paste". Errors are
    /// logged and surface as an empty result so the state machine resets.
    pub fn spawn<F, R>(factory: F, on_result: R) -> Self
    where
        F: FnOnce() -> Result<Box<dyn Transcriber>> + Send + 'static,
        R: Fn(String) + Send + 'static,
    {
        let (tx, rx) = mpsc::channel::<Vec<i16>>();
        let handle = std::thread::Builder::new()
            .name("ascuta-stt".into())
            .spawn(move || {
                let mut engine = match factory() {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("ascuta: STT engine failed to load: {e:#}");
                        while rx.recv().is_ok() {
                            on_result(String::new());
                        }
                        return;
                    }
                };
                while let Ok(pcm) = rx.recv() {
                    let samples: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
                    let text = match engine.transcribe(&samples) {
                        Ok(t) => crate::stt::tidy_transcript(&t),
                        Err(e) => {
                            eprintln!("ascuta: transcription failed: {e:#}");
                            String::new()
                        }
                    };
                    on_result(text);
                }
            })
            .expect("spawn stt worker");
        Self {
            tx: Some(tx),
            handle: Some(handle),
        }
    }

    pub fn submit(&self, pcm: Vec<i16>) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.send(pcm).is_ok())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// sherpa-onnx backed engine (Parakeet transducer / Whisper), CPU only.
#[cfg(feature = "sherpa")]
pub mod sherpa {
    use super::Transcriber;
    use crate::models::{self, SttFamily};
    use anyhow::{Context, Result};
    use std::path::Path;

    pub struct SherpaTranscriber {
        inner: Inner,
    }

    enum Inner {
        Transducer(sherpa_rs::transducer::TransducerRecognizer),
        Whisper(sherpa_rs::whisper::WhisperRecognizer),
    }

    fn pick(dir: &Path, needle: &str, required: &[&str]) -> Result<String> {
        let f = required
            .iter()
            .find(|f| f.contains(needle))
            .with_context(|| format!("model spec has no `{needle}` file"))?;
        Ok(dir.join(f).to_string_lossy().into_owned())
    }

    impl SherpaTranscriber {
        pub fn load(
            models_dir: &Path,
            model_id: &str,
            language: &str,
            threads: i32,
        ) -> Result<Self> {
            let spec = models::find_model(model_id).context("unknown model")?;
            let dir = models_dir.join(spec.extracted_dir);
            let req = spec.required_files;
            let inner = match spec.family {
                SttFamily::Parakeet => {
                    let cfg = crate::stt::RecognizerConfig::parakeet(model_id, dir.clone());
                    cfg.assert_files_exist()?;
                    Inner::Transducer(
                        sherpa_rs::transducer::TransducerRecognizer::new(
                            sherpa_rs::transducer::TransducerConfig {
                                encoder: pick(&dir, "encoder", req)?,
                                decoder: pick(&dir, "decoder", req)?,
                                joiner: pick(&dir, "joiner", req)?,
                                tokens: pick(&dir, "tokens", req)?,
                                num_threads: threads,
                                sample_rate: cfg.sample_rate as i32,
                                feature_dim: cfg.feature_dim as i32,
                                decoding_method: cfg.decoding_method.to_string(),
                                model_type: "nemo_transducer".into(),
                                provider: Some(cfg.provider.to_string()),
                                ..Default::default()
                            },
                        )
                        .map_err(|e| anyhow::anyhow!("{e}"))?,
                    )
                }
                SttFamily::Llm => anyhow::bail!("{model_id} is not a speech model"),
                SttFamily::Whisper => Inner::Whisper(
                    sherpa_rs::whisper::WhisperRecognizer::new(
                        sherpa_rs::whisper::WhisperConfig {
                            encoder: pick(&dir, "encoder", req)?,
                            decoder: pick(&dir, "decoder", req)?,
                            tokens: pick(&dir, "tokens", req)?,
                            language: if language == "auto" {
                                String::new()
                            } else {
                                language.to_string()
                            },
                            num_threads: Some(threads),
                            provider: Some("cpu".into()),
                            ..Default::default()
                        },
                    )
                    .map_err(|e| anyhow::anyhow!("{e}"))?,
                ),
            };
            Ok(Self { inner })
        }
    }

    impl Transcriber for SherpaTranscriber {
        fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
            Ok(match &mut self.inner {
                Inner::Transducer(r) => r.transcribe(16_000, samples),
                Inner::Whisper(r) => r.transcribe(16_000, samples).text,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn worker_returns_tidy_text() {
        let (tx, rx) = mpsc::channel();
        let w = Worker::spawn(
            || {
                Ok(Box::new(FakeTranscriber {
                    reply: "  hello   world ".into(),
                }) as Box<dyn Transcriber>)
            },
            move |t| {
                let _ = tx.send(t);
            },
        );
        assert!(w.submit(vec![0; 1600]));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "Hello world."
        );
    }

    #[test]
    fn failed_load_yields_empty_results() {
        let (tx, rx) = mpsc::channel();
        let w = Worker::spawn(
            || anyhow::bail!("no model"),
            move |t| {
                let _ = tx.send(t);
            },
        );
        assert!(w.submit(vec![0; 10]));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "");
    }
}
