use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::manifest::{fetch_binary, Binary};

/// Download a binary to a temp path, verify its SHA-256, then atomically move
/// it into place. Bails before touching `dest` if the checksum fails.
pub fn fetch_and_place(binary: &Binary, dest: &Path) -> Result<()> {
    println!("  downloading {}…", binary.name);
    let bytes = fetch_binary(&binary.dl_url, &binary.github_url)
        .with_context(|| format!("downloading {}", binary.name))?;

    verify_sha256(&bytes, &binary.sha256)
        .with_context(|| format!("checksum mismatch for {}", binary.name))?;

    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, &bytes).context("writing binary to tmp")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }

    std::fs::rename(&tmp, dest).context("placing binary")?;
    println!("  installed {}", dest.display());
    Ok(())
}

fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<()> {
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
}
