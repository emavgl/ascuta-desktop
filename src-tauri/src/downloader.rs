//! Model download staging — logic companion to Paseo
//! `packages/server/src/server/speech/providers/local/sherpa/model-downloader.ts`
//! (`ensureSherpaOnnxModels`).
//!
//! Staging paths, archive-type detection and the completion probe (same
//! `requiredFiles` rule as `models::missing_files`), plus the blocking
//! fetcher (`ureq`) with progress callback and temp-then-rename install.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    TarBz2,
    TarGz,
    Zip,
    /// A bare file (e.g. a `.gguf`), saved under its own name.
    SingleFile,
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
    } else if u.ends_with(".gguf") || u.ends_with(".litertlm") {
        ArchiveKind::SingleFile
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

/// Reader adapter reporting bytes consumed to a progress callback.
struct ProgressReader<'a, R> {
    inner: R,
    done: u64,
    total: Option<u64>,
    cb: &'a mut dyn FnMut(u64, Option<u64>),
}

impl<R: Read> Read for ProgressReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.done += n as u64;
        (self.cb)(self.done, self.total);
        Ok(n)
    }
}

/// Unpack a tar-based archive stream into `dest` (created if needed).
pub fn extract_archive(reader: impl Read, kind: ArchiveKind, dest: &Path) -> Result<()> {
    extract_named(reader, kind, dest, None)
}

/// Like [`extract_archive`]; `file_name` names the output for `SingleFile`.
fn extract_named(
    mut reader: impl Read,
    kind: ArchiveKind,
    dest: &Path,
    file_name: Option<&str>,
) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    match kind {
        ArchiveKind::SingleFile => {
            let name = file_name.context("single-file download needs a file name")?;
            let mut out = std::fs::File::create(dest.join(name))?;
            std::io::copy(&mut reader, &mut out)?;
        }
        ArchiveKind::TarBz2 => {
            tar::Archive::new(bzip2::read::BzDecoder::new(reader)).unpack(dest)?
        }
        ArchiveKind::TarGz => {
            tar::Archive::new(flate2::read::GzDecoder::new(reader)).unpack(dest)?
        }
        ArchiveKind::Zip | ArchiveKind::Unknown => bail!("unsupported archive type"),
    }
    Ok(())
}

/// Move the extracted tree to its final place. Archives normally contain a
/// single top-level `<extracted_dir>/`; if not, the staging dir itself is used.
fn install(staging: &Path, extracted_dir: &str, dest: &Path) -> Result<()> {
    let inner = staging.join(extracted_dir);
    let src = if inner.is_dir() {
        inner
    } else {
        staging.to_path_buf()
    };
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&src, dest).with_context(|| format!("install {}", dest.display()))?;
    let _ = std::fs::remove_dir_all(staging);
    Ok(())
}

/// SHA-256 over the bytes as they arrive, so verification never needs a second
/// copy of a multi-gigabyte archive on disk.
struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

impl<R> HashingReader<R> {
    fn finish(self) -> String {
        self.hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// The rolling sherpa-onnx release manifest: `<asset>\t<sha256>` per line.
const SHERPA_CHECKSUMS_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/checksum.txt";

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parse the release `checksum.txt` into `asset name -> lowercase sha256`.
/// Comments, blank lines and malformed entries are ignored (same rules as the
/// Android app's `parseSherpaChecksums`).
pub fn parse_sherpa_checksums(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(name), Some(hash)) = (fields.next(), fields.next()) else {
            continue;
        };
        if !name.is_empty() && is_sha256_hex(hash) {
            out.insert(name.to_string(), hash.to_ascii_lowercase());
        }
    }
    out
}

/// The hash the upstream release publishes for `asset` right now, or `None`
/// when the manifest cannot be fetched (offline) or does not list it.
fn live_sherpa_checksum(asset: &str) -> Option<String> {
    let text = ureq::get(SHERPA_CHECKSUMS_URL)
        .timeout(Duration::from_secs(20))
        .call()
        .ok()?
        .into_string()
        .ok()?;
    parse_sherpa_checksums(&text).remove(asset)
}

/// True when `actual` matches the pinned catalog hash or the live manifest.
fn checksum_accepted(actual: &str, pinned: Option<&str>, live: Option<&str>) -> bool {
    let a = actual.to_ascii_lowercase();
    pinned.is_some_and(|p| p.eq_ignore_ascii_case(&a))
        || live.is_some_and(|l| l.eq_ignore_ascii_case(&a))
}

/// Download + extract + verify a model (Paseo `ensureSherpaOnnxModels`).
/// `progress(done_bytes, total_bytes)` is called as data arrives.
pub fn download_model(
    models_dir: &Path,
    model_id: &str,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<()> {
    let plan =
        plan_for(models_dir, model_id).with_context(|| format!("unknown model id: {model_id}"))?;
    if is_complete(models_dir, model_id) {
        return Ok(());
    }
    // Per-process staging dir: a second instance (or a retry) downloading the
    // same model must never delete the files this one is extracting.
    let spec = crate::models::find_model(model_id).expect("plan implies known model");
    let staging = models_dir.join(".staging").join(format!(
        "{}.{}.part",
        spec.extracted_dir,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    let result = (|| -> Result<()> {
        let resp = ureq::get(&plan.url)
            .call()
            .with_context(|| format!("download {}", plan.url))?;
        let total = resp
            .header("Content-Length")
            .and_then(|v| v.parse::<u64>().ok());
        let file_name = plan.url.rsplit('/').next();
        let asset = file_name
            .map(str::to_string)
            .unwrap_or_else(|| plan.model_id.clone());
        let mut reader = HashingReader {
            inner: ProgressReader {
                inner: resp.into_reader(),
                done: 0,
                total,
                cb: progress,
            },
            hasher: Sha256::new(),
        };
        extract_named(&mut reader, plan.archive, &staging, file_name)?;
        // The tar/bzip2 decoders may stop at the archive trailer; hash the rest
        // of the response too so the digest always covers the whole asset.
        std::io::copy(&mut reader, &mut std::io::sink())?;
        let actual = reader.finish();

        // The `.litertlm` files live on Hugging Face (pinned commit, no rolling
        // manifest). Sherpa archives also consult the release `checksum.txt`, so
        // an upstream rebuild does not fail every install on a stale pin.
        let live = if spec.family == crate::models::SttFamily::Llm {
            None
        } else {
            live_sherpa_checksum(&asset)
        };
        if let (Some(pinned), Some(upstream)) = (spec.sha256, live.as_deref()) {
            if !pinned.eq_ignore_ascii_case(upstream) {
                eprintln!(
                    "ascuta: {asset}: checksum drift (catalog={pinned} upstream={upstream}); \
                     accepting either"
                );
            }
        }
        if spec.sha256.is_none() && live.is_none() {
            eprintln!("ascuta: {asset}: no checksum available, installing unverified");
        } else if !checksum_accepted(&actual, spec.sha256, live.as_deref()) {
            bail!(
                "checksum mismatch for {asset}: expected {}, got {actual}",
                spec.sha256
                    .map(str::to_string)
                    .or(live)
                    .unwrap_or_else(|| "?".into())
            );
        }
        if is_complete(models_dir, model_id) {
            return Ok(()); // another instance finished first; keep theirs
        }
        install(&staging, spec.extracted_dir, &plan.dest_dir)?;
        let missing = crate::models::missing_files(models_dir, model_id);
        if !missing.is_empty() {
            bail!("archive is missing expected files: {}", missing.join(", "));
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result?;
    Ok(())
}

/// Shared download state, polled by the settings GUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DlStatus {
    Downloading { done: u64, total: Option<u64> },
    Failed(String),
}

#[derive(Clone, Default)]
pub struct Tracker {
    inner: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, DlStatus>>>,
}

impl Tracker {
    pub fn status(&self, model_id: &str) -> Option<DlStatus> {
        self.inner.lock().unwrap().get(model_id).cloned()
    }

    fn set(&self, model_id: &str, st: Option<DlStatus>) {
        let mut m = self.inner.lock().unwrap();
        match st {
            Some(s) => m.insert(model_id.to_string(), s),
            None => m.remove(model_id),
        };
    }

    /// Start a background download. Returns `false` if one is already
    /// running for this model or the model is unknown. `on_done` runs after
    /// a successful install.
    pub fn start(
        &self,
        models_dir: PathBuf,
        model_id: &str,
        on_done: impl FnOnce() + Send + 'static,
    ) -> bool {
        if crate::models::find_model(model_id).is_none() {
            return false;
        }
        {
            let mut m = self.inner.lock().unwrap();
            if matches!(m.get(model_id), Some(DlStatus::Downloading { .. })) {
                return false;
            }
            m.insert(
                model_id.to_string(),
                DlStatus::Downloading {
                    done: 0,
                    total: None,
                },
            );
        }
        let (me, model) = (self.clone(), model_id.to_string());
        let needs_runtime = crate::models::find_model(model_id)
            .is_some_and(|m| m.family == crate::models::SttFamily::Llm);
        std::thread::spawn(move || {
            if needs_runtime {
                if let Err(e) = crate::cleanup::ensure_runtime() {
                    eprintln!("ascuta: cleanup runtime setup failed: {e:#}");
                    me.set(&model, Some(DlStatus::Failed(format!("{e:#}"))));
                    return;
                }
            }
            let mut last_pct = u64::MAX;
            let res = download_model(&models_dir, &model, &mut |done, total| {
                me.set(&model, Some(DlStatus::Downloading { done, total }));
                if let Some(t) = total.filter(|t| *t > 0) {
                    let pct = done * 100 / t / 10 * 10;
                    if pct != last_pct {
                        last_pct = pct;
                        eprintln!("ascuta: downloading {model}: {pct}%");
                    }
                }
            });
            match res {
                Ok(()) => {
                    me.set(&model, None);
                    eprintln!("ascuta: model {model} ready");
                    on_done();
                }
                Err(e) => {
                    eprintln!("ascuta: model download failed: {e:#}");
                    me.set(&model, Some(DlStatus::Failed(format!("{e:#}"))));
                }
            }
        });
        true
    }
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

    fn tar_gz_with(dir: &str, files: &[&str]) -> Vec<u8> {
        let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut b = tar::Builder::new(enc);
        for f in files {
            let data = b"content";
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, format!("{dir}/{f}"), &data[..])
                .unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn extracts_and_installs_archive() {
        let base = tempfile::tempdir().unwrap();
        let spec = crate::models::PARAKEET_V2_INT8;
        let bytes = tar_gz_with(spec.extracted_dir, spec.required_files);
        let staging = staging_dir(base.path(), spec.extracted_dir);
        extract_archive(&bytes[..], ArchiveKind::TarGz, &staging).unwrap();
        let dest = base.path().join(spec.extracted_dir);
        install(&staging, spec.extracted_dir, &dest).unwrap();
        assert!(is_complete(base.path(), spec.id));
        assert!(!staging.exists());
    }

    #[test]
    fn single_file_download_is_saved_by_name() {
        let out = tempfile::tempdir().unwrap();
        extract_named(
            &b"gguf-bytes"[..],
            ArchiveKind::SingleFile,
            out.path(),
            Some("m.gguf"),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(out.path().join("m.gguf")).unwrap(),
            b"gguf-bytes"
        );
        assert_eq!(archive_kind("https://x/y/m.gguf"), ArchiveKind::SingleFile);
    }

    #[test]
    fn extracts_bz2() {
        use std::io::Write;
        let mut tar_bytes = Vec::new();
        {
            let mut b = tar::Builder::new(&mut tar_bytes);
            let mut h = tar::Header::new_gnu();
            h.set_size(2);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, "d/a.txt", &b"hi"[..]).unwrap();
            b.finish().unwrap();
        }
        let mut enc = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        enc.write_all(&tar_bytes).unwrap();
        let bz = enc.finish().unwrap();
        let out = tempfile::tempdir().unwrap();
        extract_archive(&bz[..], ArchiveKind::TarBz2, out.path()).unwrap();
        assert_eq!(std::fs::read(out.path().join("d/a.txt")).unwrap(), b"hi");
    }

    #[test]
    fn unsupported_archive_errors() {
        let out = tempfile::tempdir().unwrap();
        assert!(extract_archive(&b""[..], ArchiveKind::Zip, out.path()).is_err());
    }

    #[test]
    fn hashing_reader_covers_every_byte() {
        let data = b"ascuta archive bytes";
        let mut reader = HashingReader {
            inner: &data[..],
            hasher: Sha256::new(),
        };
        let mut head = [0u8; 5];
        reader.read_exact(&mut head).unwrap();
        std::io::copy(&mut reader, &mut std::io::sink()).unwrap();
        let actual = reader.finish();

        let mut expected = Sha256::new();
        expected.update(data);
        let expected: String = expected
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn checksum_manifest_parsing() {
        let text = "# generated by sherpa-onnx\n\
                    sherpa-onnx-whisper-base.tar.bz2\t911B2083EFD7C0DCA2AC3B358B75222660DC09FB716D64FBFc417ba6c99ff3de\n\
                    malformed-line-without-a-hash\n\
                    too-short abcdef\n";
        let parsed = parse_sherpa_checksums(text);
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed["sherpa-onnx-whisper-base.tar.bz2"],
            "911b2083efd7c0dca2ac3b358b75222660dc09fb716d64fbfc417ba6c99ff3de"
        );
    }

    #[test]
    fn checksum_accepts_pinned_or_live() {
        let actual = "aa".repeat(32);
        let other = "bb".repeat(32);
        assert!(checksum_accepted(&actual, Some(&actual), None));
        assert!(checksum_accepted(&actual, None, Some(&actual)));
        assert!(checksum_accepted(&actual, Some(&other), Some(&actual)));
        assert!(!checksum_accepted(&actual, Some(&other), None));
        assert!(!checksum_accepted(&actual, None, None));
    }
}
