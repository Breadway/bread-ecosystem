//! Model download + integrity checking.
//!
//! `breadarrd/src/matcher/mod.rs::download` (async, `reqwest`) and
//! `breadmill/src/main.rs::download_if_missing` (sync, `ureq`) independently
//! implement "download to a temp file, then rename over the destination"
//! for fetching an ONNX model/tokenizer if it isn't already present —
//! genuinely duplicated intent, different HTTP clients. Neither verifies
//! the download's integrity beyond "the response wasn't empty". This module
//! is a fresh, shared implementation (sync, `ureq` — matching this
//! workspace's existing `bakery` convention for downloads) that adds an
//! optional SHA-256 check, built on [`bread_utils::atomic::write_atomic_bytes`]
//! for the same crash-safety property both originals already had.
//!
//! `breadarrd`'s async caller should wrap a call to [`ensure_file`] in
//! `tokio::task::spawn_blocking` rather than block its async runtime
//! directly — see that crate's migration for the concrete pattern.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Download `url` to `dest` if `dest` doesn't already exist. If
/// `expected_sha256` is given, verifies the downloaded bytes against it
/// (case-insensitive hex) before the atomic rename and returns an error on
/// mismatch — the temp file is discarded, `dest` is left untouched. An
/// already-present `dest` is trusted as-is and not re-verified (matches
/// both original implementations' "if it exists, skip" behavior; re-hashing
/// a ~90MB+ model file on every startup would be wasted work for the common
/// case of a stable, previously-verified file).
pub fn ensure_file(url: &str, dest: &Path, expected_sha256: Option<&str>) -> anyhow::Result<PathBuf> {
    if dest.exists() {
        return Ok(dest.to_path_buf());
    }

    tracing::info!("bread-onnx: downloading {url} -> {}", dest.display());
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(300))
        .build();
    let response = agent
        .get(url)
        .call()
        .map_err(|e| anyhow::anyhow!("failed to download {url}: {e}"))?;

    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| anyhow::anyhow!("failed to read response body from {url}: {e}"))?;

    if bytes.is_empty() {
        anyhow::bail!("empty download from {url}");
    }

    if let Some(expected) = expected_sha256 {
        let actual = sha256_hex(&bytes);
        if !actual.eq_ignore_ascii_case(expected) {
            anyhow::bail!(
                "checksum mismatch for {url}: expected {expected}, got {actual} — refusing to install"
            );
        }
        tracing::info!("bread-onnx: verified sha256 for {}", dest.display());
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    bread_utils::atomic::write_atomic_bytes(dest, &bytes, None)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", dest.display()))?;

    tracing::info!(
        "bread-onnx: saved {} ({:.1} MB)",
        dest.display(),
        bytes.len() as f64 / 1_048_576.0
    );
    Ok(dest.to_path_buf())
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_matches_known_vector() {
        // sha256("") — well-known empty-input digest.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn ensure_file_skips_download_when_already_present() {
        let dir = std::env::temp_dir().join(format!("bread-onnx-download-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("model.onnx");
        std::fs::write(&dest, b"already here").unwrap();

        // A bogus URL would fail if actually requested — success here proves
        // the existing-file short-circuit fired instead of dialing out.
        let result = ensure_file("http://127.0.0.1:1/unreachable", &dest, None);
        assert!(result.is_ok());
        assert_eq!(std::fs::read(&dest).unwrap(), b"already here");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
