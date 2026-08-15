use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::manifest::{fetch_binary, Binary};
use crate::ui;

/// Download a binary, verify its SHA-256, then atomically write it into
/// place (fsynced, temp-in-same-dir-with-unique-name then rename — see
/// `bread_utils::atomic`). Bails before touching `dest` if the checksum
/// fails. Returns the verified hex sha256 so callers (`install::
/// install_package`) can record it for `bakery verify` without hashing the
/// bytes a second time — `verify_sha256` already confirmed `bytes` matches
/// `binary.sha256`, so that's the value to return.
pub fn fetch_and_place(binary: &Binary, dest: &Path) -> Result<String> {
    ui::step("downloading", &binary.name);
    let bytes = fetch_binary(&binary.dl_url, &binary.github_url)
        .with_context(|| format!("downloading {}", binary.name))?;

    verify_sha256(&bytes, &binary.sha256)
        .with_context(|| format!("checksum mismatch for {}", binary.name))?;

    bread_utils::atomic::write_atomic_bytes(dest, &bytes, Some(0o755))
        .with_context(|| format!("placing binary at {}", dest.display()))?;
    ui::step("placed", &dest.display().to_string());
    Ok(binary.sha256.clone())
}

/// Verify that `bytes` hashes to `expected_hex` under SHA-256.
///
/// Shared by every artifact download path — binaries (via
/// [`fetch_and_place`]), and config-example / systemd-unit downloads in
/// `install.rs` — so all downloaded artifacts get the same integrity check.
pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<()> {
    if expected_hex.is_empty() {
        bail!("index entry has no sha256 recorded — refusing to trust an unverifiable download");
    }
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual = hex::encode(hasher.finalize());
    if actual != expected_hex {
        bail!(
            "SHA-256 mismatch\n  expected: {}\n  actual:   {}",
            expected_hex,
            actual
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn sha256_hex(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    #[test]
    fn verify_correct_hash() {
        let bytes = b"hello bakery";
        let hash = sha256_hex(bytes);
        assert!(verify_sha256(bytes, &hash).is_ok());
    }

    #[test]
    fn verify_wrong_hash_fails() {
        let bytes = b"hello bakery";
        let wrong = "0".repeat(64);
        assert!(verify_sha256(bytes, &wrong).is_err());
    }

    #[test]
    fn verify_empty_bytes() {
        let bytes = b"";
        let hash = sha256_hex(bytes);
        assert!(verify_sha256(bytes, &hash).is_ok());
    }

    #[test]
    fn verify_missing_sha256_gives_a_clear_error() {
        // gen-index.sh emits an empty sha256 string when a .sha256 sidecar
        // is missing — must not fall through to the generic mismatch
        // message ("expected: \n actual: <hex>"), which is confusing about
        // what actually went wrong.
        let err = verify_sha256(b"anything", "").unwrap_err();
        assert!(err.to_string().contains("no sha256 recorded"));
    }
}
