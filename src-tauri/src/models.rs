//! Local speech model catalog — 1:1 port of Paseo
//! `packages/server/src/server/speech/providers/local/sherpa/model-catalog.ts`
//! plus Whisper entries for the ascuta desktop scope (PLAN §5).
//!
//! IDs are stable strings used in `settings.model` and on disk under
//! `<modelsDir>/<extractedDir>/`. URLs point at the k2-fsa sherpa-onnx
//! release buckets (STT) — downloads happen in `downloader` (M2 follow-up).

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelKind {
    SttOffline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SttFamily {
    /// NVIDIA NeMo transducer (FastConformer encoder + TDT decoder).
    Parakeet,
    /// OpenAI Whisper exported to ONNX (encoder-decoder).
    Whisper,
    /// Text LLM (`.litertlm`, LiteRT-LM) used for transcript cleanup.
    Llm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    pub id: &'static str,
    pub family: SttFamily,
    pub kind: ModelKind,
    pub archive_url: &'static str,
    pub extracted_dir: &'static str,
    pub required_files: &'static [&'static str],
    pub description: &'static str,
    pub default_for_en: bool,
    /// Lower-case hex SHA-256 of the downloaded **archive bytes** (or the
    /// `.litertlm` file), checked before extracting/installing. The downloader
    /// also reads the sherpa-onnx release `checksum.txt` at download time and
    /// accepts either value, so an upstream rebuild cannot make every install
    /// fail on a stale pin. `None` = unverified (the catalog test forbids it).
    pub sha256: Option<&'static str>,
}

pub const PARAKEET_V2_INT8: ModelSpec = ModelSpec {
    id: "parakeet-tdt-0.6b-v2-int8",
    family: SttFamily::Parakeet,
    kind: ModelKind::SttOffline,
    archive_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2",
    extracted_dir: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8",
    required_files: &[
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "joiner.int8.onnx",
        "tokens.txt",
    ],
    description: "NVIDIA Parakeet TDT v2 (offline NeMo transducer, English).",
    default_for_en: true,
    sha256: Some("6fc4426b54aa549a0019b8a75b8dea59164e71790a37ac56a045ad57f915f714"),
};

pub const PARAKEET_V3_INT8: ModelSpec = ModelSpec {
    id: "parakeet-tdt-0.6b-v3-int8",
    family: SttFamily::Parakeet,
    kind: ModelKind::SttOffline,
    archive_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
    extracted_dir: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
    required_files: &[
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "joiner.int8.onnx",
        "tokens.txt",
    ],
    description: "NVIDIA Parakeet TDT v3 (offline NeMo transducer, 25 European languages).",
    default_for_en: false,
    sha256: Some("1495e91b59d7fc53ddca91f3431758ec99f8fa253fdb0e1e851be0f72b14b0f9"),
};

pub const WHISPER_BASE_INT8: ModelSpec = ModelSpec {
    id: "whisper-base-int8",
    family: SttFamily::Whisper,
    kind: ModelKind::SttOffline,
    archive_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-base.tar.bz2",
    extracted_dir: "sherpa-onnx-whisper-base",
    required_files: &[
        "base-encoder.int8.onnx",
        "base-decoder.int8.onnx",
        "base-tokens.txt",
    ],
    description: "Whisper base int8 (99+ languages, fallback for non-EU).",
    default_for_en: false,
    sha256: Some("911b2083efd7c0dca2ac3b358b75222660dc09fb716d64fbfc417ba6c99ff3de"),
};

pub const WHISPER_SMALL_INT8: ModelSpec = ModelSpec {
    id: "whisper-small-int8",
    family: SttFamily::Whisper,
    kind: ModelKind::SttOffline,
    archive_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-small.tar.bz2",
    extracted_dir: "sherpa-onnx-whisper-small",
    required_files: &[
        "small-encoder.int8.onnx",
        "small-decoder.int8.onnx",
        "small-tokens.txt",
    ],
    description: "Whisper small int8 (99+ languages, better accuracy, ~500MB).",
    default_for_en: false,
    sha256: Some("486a46afbb7ba798507190ffe02fea2dd726049af212e774537efac6afb210a6"),
};

pub const WHISPER_LARGE_V3_TURBO_INT8: ModelSpec = ModelSpec {
    id: "whisper-large-v3-turbo-int8",
    family: SttFamily::Whisper,
    kind: ModelKind::SttOffline,
    archive_url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-whisper-turbo.tar.bz2",
    extracted_dir: "sherpa-onnx-whisper-turbo",
    required_files: &[
        "turbo-encoder.int8.onnx",
        "turbo-decoder.int8.onnx",
        "turbo-tokens.txt",
    ],
    description: "Whisper large-v3-turbo int8 (accuracy-first, ~1GB, warn on RAM).",
    default_for_en: false,
    sha256: Some("b11acbbcd660b44a8e0df33724feb5aaa709cf65668f2823d59f656312544f22"),
};

/// Gemma 4 E2B for LiteRT-LM — the exact file the Android app downloads and
/// runs for its cleanup pass (litert-community build, ~2.6 GB).
pub const GEMMA_4_E2B_LITERT: ModelSpec = ModelSpec {
    id: "gemma-4-e2b-it-litertlm",
    family: SttFamily::Llm,
    kind: ModelKind::SttOffline,
    archive_url: "https://huggingface.co/litert-community/gemma-4-E2B-it-litert-lm/resolve/2d74ac41a33d7da82f39f7d5720a04c68514a103/gemma-4-E2B-it.litertlm",
    extracted_dir: "gemma-4-E2B-it-litert-lm",
    required_files: &["gemma-4-E2B-it.litertlm"],
    description: "Gemma 4 E2B via LiteRT-LM (transcript cleanup, ~2.6 GB, optional).",
    default_for_en: false,
    sha256: Some("181938105e0eefd105961417e8da75903eacda102c4fce9ce90f50b97139a63c"),
};

/// Optional text-cleanup models (not speech recognizers).
pub const LLM_MODELS: &[ModelSpec] = &[GEMMA_4_E2B_LITERT];
pub const DEFAULT_CLEANUP_MODEL: &str = "gemma-4-e2b-it-litertlm";

pub const ALL_MODELS: &[ModelSpec] = &[
    PARAKEET_V2_INT8,
    PARAKEET_V3_INT8,
    WHISPER_BASE_INT8,
    WHISPER_SMALL_INT8,
    WHISPER_LARGE_V3_TURBO_INT8,
];

pub const DEFAULT_STT_MODEL: &str = "parakeet-tdt-0.6b-v2-int8";

/// Bundled VAD model file (shipped with the app, not downloaded).
pub const SILERO_VAD_ASSET: &str = "silero_vad.onnx";

/// Any downloadable model (speech or cleanup).
pub fn find_model(id: &str) -> Option<ModelSpec> {
    ALL_MODELS
        .iter()
        .chain(LLM_MODELS.iter())
        .find(|m| m.id == id)
        .copied()
}

/// A speech-recognition model id (what `settings.model` may hold).
pub fn is_known_model(id: &str) -> bool {
    ALL_MODELS.iter().any(|m| m.id == id)
}

/// Resolve `<modelsDir>/<extractedDir>` for a model id.
/// Mirrors Paseo `getLocalSpeechModelDir()`.
pub fn model_dir(models_dir: &std::path::Path, id: &str) -> Option<PathBuf> {
    find_model(id).map(|m| models_dir.join(m.extracted_dir))
}

/// Default models dir: `$ASCUTA_MODELS_DIR`, else per-OS data dir
/// (`~/.local/share/ascuta/models` on Linux). Mirrors Paseo
/// `PASEO_LOCAL_MODELS_DIR` / `~/paseo/models/local-speech`.
pub fn default_models_dir() -> PathBuf {
    if let Ok(p) = std::env::var("ASCUTA_MODELS_DIR") {
        return PathBuf::from(p);
    }
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("ascuta").join("models")
}

/// Check that all `required_files` exist and are non-empty (dirs count as
/// present, mirroring Paseo `hasRequiredLocalModelFile`).
pub fn missing_files(models_dir: &std::path::Path, id: &str) -> Vec<String> {
    let Some(spec) = find_model(id) else {
        return vec![format!("unknown model id: {id}")];
    };
    let dir = models_dir.join(spec.extracted_dir);
    spec.required_files
        .iter()
        .filter(|rel| {
            let p = dir.join(rel);
            match std::fs::metadata(&p) {
                Ok(md) => {
                    if md.is_dir() {
                        false
                    } else {
                        md.len() == 0
                    }
                }
                Err(_) => true,
            }
        })
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_parakeet_v2() {
        assert_eq!(DEFAULT_STT_MODEL, "parakeet-tdt-0.6b-v2-int8");
        let spec = find_model(DEFAULT_STT_MODEL).expect("default in catalog");
        assert!(spec.default_for_en);
        assert!(is_known_model(DEFAULT_STT_MODEL));
    }

    #[test]
    fn unknown_model_reported() {
        assert!(!is_known_model("nope"));
        assert_eq!(
            missing_files(PathBuf::from("/tmp").as_path(), "nope").len(),
            1
        );
    }

    #[test]
    fn missing_files_detected_in_empty_tmp() {
        let dir = std::env::temp_dir().join(format!("ascuta-models-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let missing = missing_files(&dir, DEFAULT_STT_MODEL);
        assert_eq!(missing.len(), PARAKEET_V2_INT8.required_files.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_override_wins() {
        std::env::set_var("ASCUTA_MODELS_DIR", "/tmp/ascuta-test-models");
        assert_eq!(
            default_models_dir(),
            PathBuf::from("/tmp/ascuta-test-models")
        );
        std::env::remove_var("ASCUTA_MODELS_DIR");
    }

    #[test]
    fn every_model_pins_a_valid_checksum() {
        for spec in ALL_MODELS.iter().chain(LLM_MODELS.iter()) {
            let hash = spec
                .sha256
                .unwrap_or_else(|| panic!("{} has no pinned sha256", spec.id));
            assert_eq!(hash.len(), 64, "{} hash length", spec.id);
            assert!(
                hash.bytes().all(|b| b.is_ascii_hexdigit()),
                "{} hash is not hex",
                spec.id
            );
            assert_eq!(hash, hash.to_ascii_lowercase(), "{} hash case", spec.id);
        }
    }

    #[test]
    fn sherpa_urls_point_at_the_rolling_release() {
        // The 404 class of bug: every speech asset must live in `asr-models`
        // with the `.tar.bz2` name the release manifest publishes.
        for spec in ALL_MODELS.iter() {
            assert!(
                spec.archive_url.contains("/asr-models/"),
                "{} is not on the asr-models release",
                spec.id
            );
            assert!(
                spec.archive_url.ends_with(".tar.bz2"),
                "{} is not a tar.bz2 asset",
                spec.id
            );
        }
    }
}
