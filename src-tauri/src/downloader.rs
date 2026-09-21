//! Model download staging — logic companion to Paseo
//! `packages/server/src/server/speech/providers/local/sherpa/model-downloader.ts`
//! (`ensureSherpaOnnxModels`).
//!
//! No network client is linked yet (M2 fast-follow adds `ureq`/progress).
//! This module owns the deterministic parts: staging paths, archive-type
//! detection, completion probe (same `requiredFiles` rule as
//! `models::missing_files`), so the fetcher drops in without behavior change.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    TarBz2,
    TarGz,
    Zip,
    Unknown,
}

pub fn archive_kind(url: &str) -> ArchiveKind {
    let u = url.to_lowercase();
    if u.ends_with(".tar.bz2") || u.ends_with(".tbz2") {
        ArchiveKind::TarBz2
    } else if u.ends_with(".tar.gz") || u.ends_with(".tgz") {
        ArchiveKind::TarGz
    } else if u.ends_with(".zip") {
        ArchiveKind::Zip
    } else {
        ArchiveKind::Unknown
    }
}

/// `<modelsDir>/.staging/<extractedDir>.part` — atomic rename target on
/// success, mirroring Paseo's temp-then-rename discipline.
pub fn staging_dir(models_dir: &Path, extracted_dir: &str) -> PathBuf {
    models_dir
        .join(".staging")
        .join(format!("{extracted_dir}.part"))
}

/// True when every `required_file` exists and is non-empty.
pub fn is_complete(models_dir: &Path, model_id: &str) -> bool {
    crate::models::missing_files(models_dir, model_id).is_empty()
}

/// Plan record surfaced to the settings UI (model picker progress row).
#[derive(Debug, Clone)]
pub struct DownloadPlan {
    pub model_id: String,
    pub url: String,
    pub dest_dir: PathBuf,
    pub staging: PathBuf,
    pub archive: ArchiveKind,
}

pub fn plan_for(models_dir: &Path, model_id: &str) -> Option<DownloadPlan> {
    let spec = crate::models::find_model(model_id)?;
    Some(DownloadPlan {
        model_id: spec.id.to_string(),
        url: spec.archive_url.to_string(),
        dest_dir: models_dir.join(spec.extracted_dir),
        staging: staging_dir(models_dir, spec.extracted_dir),
        archive: archive_kind(spec.archive_url),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_sniffing() {
        assert_eq!(
            archive_kind("https://x/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2"),
            ArchiveKind::TarBz2
        );
        assert_eq!(archive_kind("https://x/m.tar.gz"), ArchiveKind::TarGz);
        assert_eq!(archive_kind("https://x/m.zip"), ArchiveKind::Zip);
        assert_eq!(archive_kind("https://x/m"), ArchiveKind::Unknown);
    }

    #[test]
    fn plan_points_at_staging() {
        let plan = plan_for(
            PathBuf::from("/m").as_path(),
            crate::models::DEFAULT_STT_MODEL,
        )
        .unwrap();
        assert!(plan.staging.starts_with("/m/.staging"));
        assert!(plan
            .dest_dir
            .ends_with(crate::models::PARAKEET_V2_INT8.extracted_dir));
        assert_eq!(plan.archive, ArchiveKind::TarBz2);
    }

    #[test]
    fn incomplete_in_empty_dir() {
        let dir = std::env::temp_dir().join(format!("ascuta-dl-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert!(!is_complete(&dir, crate::models::DEFAULT_STT_MODEL));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn complete_when_files_present() {
        let base = std::env::temp_dir().join(format!("ascuta-dl-ok-{}", std::process::id()));
        let dir = base.join(crate::models::PARAKEET_V2_INT8.extracted_dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in crate::models::PARAKEET_V2_INT8.required_files {
            std::fs::write(dir.join(f), b"stub").unwrap();
        }
        assert!(is_complete(&base, crate::models::DEFAULT_STT_MODEL));
        let _ = std::fs::remove_dir_all(&base);
    }
}
