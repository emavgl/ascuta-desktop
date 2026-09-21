//! STT session assembly — logic port of Paseo
//! `packages/server/src/server/agent/stt-manager.ts`
//! (`assembleTranscriptionResult`, commit cadence, low-confidence gate)
//! plus the sherpa recognizer config from
//! `sherpa-offline-recognizer.ts`.
//!
//! The native `sherpa-onnx` call itself is NOT linked yet (needs the
//! prebuilt C-API lib per OS). This module owns everything around it so
//! the worker thread (M2) drops in with zero behavior change.

use std::collections::{HashMap, HashSet};

/// Mirrors `SherpaOfflineRecognizerConfig` in Paseo.
#[derive(Debug, Clone)]
pub struct RecognizerConfig {
    pub model_id: String,
    pub model_dir: std::path::PathBuf,
    pub sample_rate: u32,
    pub feature_dim: u32,
    pub num_threads: u32,
    pub provider: &'static str,
    pub decoding_method: &'static str,
}

impl RecognizerConfig {
    pub fn parakeet(model_id: &str, model_dir: std::path::PathBuf) -> Self {
        Self {
            model_id: model_id.to_string(),
            model_dir,
            sample_rate: 16_000,
            feature_dim: 80,
            num_threads: 2,  // Paseo default in worker-process.ts
            provider: "cpu", // GPU opt-in later, PLAN §6
            decoding_method: "greedy_search",
        }
    }

    /// `encoder/decoder/joiner.int8.onnx + tokens.txt` — same layout Paseo
    /// asserts in `SherpaOfflineRecognizerEngine`.
    pub fn required_paths(&self) -> [std::path::PathBuf; 4] {
        [
            self.model_dir.join("encoder.int8.onnx"),
            self.model_dir.join("decoder.int8.onnx"),
            self.model_dir.join("joiner.int8.onnx"),
            self.model_dir.join("tokens.txt"),
        ]
    }

    pub fn assert_files_exist(&self) -> anyhow::Result<()> {
        for p in self.required_paths() {
            if !p.exists() {
                anyhow::bail!("Missing model file: {}", p.display());
            }
        }
        Ok(())
    }
}

/// Paseo `DEFAULT_BATCH_COMMIT_EVERY_SECONDS` (15s) + 1s append chunks.
pub const APPEND_CHUNK_SECONDS: u32 = 1;
pub const DEFAULT_COMMIT_EVERY_SECONDS: f64 = 15.0;

pub fn commit_every_seconds() -> f64 {
    std::env::var("ASCUTA_STT_COMMIT_SECONDS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(DEFAULT_COMMIT_EVERY_SECONDS)
}

/// Paseo `STT_CONFIDENCE_THRESHOLD` default (-3.0): below → empty transcript.
pub const DEFAULT_CONFIDENCE_THRESHOLD: f64 = -3.0;

/// Assemble final text from committed segment ids + per-segment transcripts.
/// Committed ids keep order; uncommitted finals append after (Paseo
/// `assembleTranscriptionResult`).
pub fn assemble_transcript(
    committed_ids: &[String],
    transcripts: &HashMap<String, String>,
    final_ids: &HashSet<String>,
) -> String {
    let committed: HashSet<&String> = committed_ids.iter().collect();
    let mut ordered: Vec<&String> = committed_ids.iter().collect();
    for id in transcripts.keys() {
        if !committed.contains(id) {
            ordered.push(id);
        }
    }
    ordered
        .iter()
        .filter(|id| final_ids.contains(**id))
        .filter_map(|id| transcripts.get(*id))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Low-confidence gate: Paseo returns `""` so the state machine aborts the
/// paste (`on_decoded(false)`).
pub fn apply_confidence_gate(text: &str, avg_logprob: Option<f64>) -> &str {
    match avg_logprob {
        Some(lp) if lp < DEFAULT_CONFIDENCE_THRESHOLD => "",
        _ => text,
    }
}

/// Deterministic tidy for Whisper outputs (Parakeet TDT already punctuates):
/// collapse whitespace, capitalize first letter, ensure terminal period.
/// Paseo does this client-side; kept here so both engines share it.
pub fn tidy_transcript(raw: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = collapsed.trim();
    if t.is_empty() {
        return String::new();
    }
    let mut chars = t.chars();
    let first = chars.next().unwrap().to_uppercase().to_string();
    let rest: String = chars.collect();
    let mut out = format!("{first}{rest}");
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembly_preserves_commit_order() {
        let committed = vec!["a".to_string(), "b".to_string()];
        let mut tx = HashMap::new();
        tx.insert("a".to_string(), "hello".to_string());
        tx.insert("b".to_string(), "world".to_string());
        let finals: HashSet<String> = ["a".to_string(), "b".to_string()].into();
        assert_eq!(assemble_transcript(&committed, &tx, &finals), "hello world");
    }

    #[test]
    fn non_final_segments_excluded() {
        let committed = vec!["a".to_string()];
        let mut tx = HashMap::new();
        tx.insert("a".to_string(), "hi".to_string());
        let finals = HashSet::new();
        assert_eq!(assemble_transcript(&committed, &tx, &finals), "");
    }

    #[test]
    fn low_confidence_empties() {
        assert_eq!(apply_confidence_gate("hello", Some(-5.0)), "");
        assert_eq!(apply_confidence_gate("hello", Some(-1.0)), "hello");
        assert_eq!(apply_confidence_gate("hello", None), "hello");
    }

    #[test]
    fn tidy_adds_caps_and_period() {
        assert_eq!(tidy_transcript("  hello   world  "), "Hello world.");
        assert_eq!(tidy_transcript("Done!"), "Done!");
        assert_eq!(tidy_transcript("   "), "");
    }

    #[test]
    fn recognizer_config_paths() {
        let cfg = RecognizerConfig::parakeet("m", "/models/m".into());
        let paths = cfg.required_paths();
        assert!(paths[0].ends_with("encoder.int8.onnx"));
        assert!(cfg.assert_files_exist().is_err());
    }
}
