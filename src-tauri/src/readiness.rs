//! Speech readiness — port of Paseo
//! `packages/server/src/server/speech/speech-runtime.ts`
//! (`buildDictationReadiness` / `buildVoiceFeatureReadiness`).
//!
//! Single-model v1: dictation readiness only. `ready` ⇔ model files present.
//! The background-download + monitor tick lands with the network fetcher.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessCode {
    Ready,
    ModelsMissing,
    UnknownModel,
}

#[derive(Debug, Clone)]
pub struct Readiness {
    pub code: ReadinessCode,
    pub message: String,
    pub missing_files: Vec<String>,
    pub retryable: bool,
}

pub fn dictation_readiness(models_dir: &std::path::Path, model_id: &str) -> Readiness {
    if crate::models::find_model(model_id).is_none() {
        return Readiness {
            code: ReadinessCode::UnknownModel,
            message: format!("Unknown STT model: {model_id}"),
            missing_files: vec![],
            retryable: false,
        };
    }
    let missing = crate::models::missing_files(models_dir, model_id);
    if missing.is_empty() {
        Readiness {
            code: ReadinessCode::Ready,
            message: "Dictation is ready.".to_string(),
            missing_files: vec![],
            retryable: false,
        }
    } else {
        Readiness {
            message: format!(
                "Dictation unavailable: missing local model files ({}) — download {} first.",
                missing.join(", "),
                model_id
            ),
            code: ReadinessCode::ModelsMissing,
            missing_files: missing,
            retryable: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_when_complete() {
        let base = std::env::temp_dir().join(format!("ascuta-rdy-{}", std::process::id()));
        let dir = base.join(crate::models::PARAKEET_V2_INT8.extracted_dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in crate::models::PARAKEET_V2_INT8.required_files {
            std::fs::write(dir.join(f), b"stub").unwrap();
        }
        let r = dictation_readiness(&base, crate::models::DEFAULT_STT_MODEL);
        assert_eq!(r.code, ReadinessCode::Ready);
        assert!(!r.retryable);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_when_empty() {
        let dir = std::env::temp_dir().join(format!("ascuta-rdy-no-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let r = dictation_readiness(&dir, crate::models::DEFAULT_STT_MODEL);
        assert_eq!(r.code, ReadinessCode::ModelsMissing);
        assert!(r.retryable);
        assert!(!r.missing_files.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_model() {
        let r = dictation_readiness(std::path::Path::new("/tmp"), "nope");
        assert_eq!(r.code, ReadinessCode::UnknownModel);
    }
}
