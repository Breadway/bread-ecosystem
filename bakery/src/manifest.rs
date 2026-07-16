use anyhow::{bail, Context, Result};
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const PRIMARY_URL: &str = "https://dl.breadway.dev/index.json";
const SIG_URL: &str = "https://dl.breadway.dev/index.json.minisig";
const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// The bakery index-signing public key.
///
/// The matching secret key is used offline (never on this machine, never in
/// this repo) to sign `index.json` with `minisign` as part of publishing a
/// new index — see `scripts/gen-index.sh`. Every fetch of `index.json`, and
/// every load of the on-disk cache, must verify against this key before the
/// bytes are trusted or parsed. This is the single control point: the
/// per-artifact `sha256` fields and `post_install` hook strings all live
/// inside `index.json` itself, so a valid signature transitively covers them.
const PUBKEY: &str = "RWRh2Zr5SUinvVFCtD7S7HwGjfrye6j31Xq2mYXRdkGFDWe3yHF7W11K";

/// Verify `bytes` against `sig_text` (the contents of an `index.json.minisig`
/// file) using the pinned [`PUBKEY`]. Returns an error on any failure —
/// missing/malformed signature, wrong key, or a hash mismatch.
fn verify_index_signature(bytes: &[u8], sig_text: &str) -> Result<()> {
    verify_against_key(bytes, sig_text, PUBKEY)
}

/// Verify `bytes` against a minisign `sig_text` using an arbitrary base64
/// public key. Split out from [`verify_index_signature`] purely so tests can
/// exercise the verification logic with a throwaway keypair instead of the
/// real production key.
fn verify_against_key(bytes: &[u8], sig_text: &str, pubkey_b64: &str) -> Result<()> {
    let public_key =
        PublicKey::from_base64(pubkey_b64).context("public key is malformed")?;
    let signature =
        Signature::decode(sig_text).context("index.json.minisig is malformed or unreadable")?;
    public_key
        .verify(bytes, &signature, false)
        .context("index.json failed signature verification against the pinned bakery key")
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Binary {
    pub name: String,
    pub dl_url: String,
    pub github_url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Service {
    pub unit: String,
    pub enable: bool,
    /// SHA-256 of the unit file artifact. Required to verify the download in
    /// `install::install_service`, same as binaries; `index.json` carries it
    /// (and is itself minisign-signed, which is what makes it trustworthy).
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfigScaffold {
    pub dir: String,
    /// Example config filename, relative to the release artifact directory.
    pub example: Option<String>,
    /// SHA-256 of the example config artifact, when `example` is set.
    /// Verified in `install::scaffold_config` the same way binaries are.
    #[serde(default)]
    pub example_sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Package {
    pub name: String,
    pub description: String,
    pub version: String,
    pub binaries: Vec<Binary>,
    #[serde(default)]
    pub system_deps: Vec<String>,
    #[serde(default)]
    pub optional_system_deps: Vec<String>,
    #[serde(default)]
    pub bread_deps: Vec<String>,
    #[serde(default)]
    pub services: Vec<Service>,
    pub config: Option<ConfigScaffold>,
    #[serde(default)]
    pub post_install: Vec<String>,
}

impl Package {
    /// Returns `(primary_url, github_url)` for any artifact filename in this
    /// package's release directory. Derived by stripping the filename from the
    /// first binary's URLs.
    pub fn artifact_urls(&self, filename: &str) -> Option<(String, String)> {
        let first = self.binaries.first()?;
        let dl_base = first.dl_url.rsplit_once('/')?.0;
        let gh_base = first.github_url.rsplit_once('/')?.0;
        Some((
            format!("{dl_base}/{filename}"),
            format!("{gh_base}/{filename}"),
        ))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Index {
    pub version: String,
    pub packages: std::collections::HashMap<String, Package>,
}

impl Index {
    pub fn get(&self, name: &str) -> Option<&Package> {
        self.packages.get(name)
    }

    #[allow(dead_code)]
    pub fn all(&self) -> impl Iterator<Item = &Package> {
        self.packages.values()
    }
}

/// Load the manifest, using the on-disk cache when it is fresh enough.
/// Always fetches if `force_refresh` is true.
///
/// Every path — fresh fetch or cached read — verifies the minisign
/// signature over the raw `index.json` bytes before the JSON is parsed or
/// trusted. A signature failure on a freshly fetched index is always a hard
/// error. A signature failure on the *cached* copy is treated as a
/// (possibly tampered, possibly just stale-format) cache and triggers one
/// re-fetch from the network rather than bricking the CLI outright; if the
/// freshly fetched copy also fails to verify, that's a hard error.
pub fn load(force_refresh: bool) -> Result<Index> {
    let cache_path = cache_path();
    let sig_cache_path = sig_cache_path(&cache_path);

    if !force_refresh && cache_is_fresh(&cache_path) {
        match read_and_verify_cache(&cache_path, &sig_cache_path) {
            Ok(index) => return Ok(index),
            Err(err) => {
                eprintln!(
                    "  warning: cached index.json failed verification ({err}), re-fetching…"
                );
            }
        }
    }

    fetch_and_cache(&cache_path, &sig_cache_path)
}

fn read_and_verify_cache(cache_path: &PathBuf, sig_cache_path: &PathBuf) -> Result<Index> {
    let bytes = std::fs::read(cache_path).context("reading cached index")?;
    let sig_text = std::fs::read_to_string(sig_cache_path)
        .context("reading cached index.json.minisig (cache predates signing support)")?;
    verify_index_signature(&bytes, &sig_text)?;
    serde_json::from_slice(&bytes).context("parsing cached index")
}

fn cache_is_fresh(path: &PathBuf) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| SystemTime::now().duration_since(t).unwrap_or(CACHE_MAX_AGE) < CACHE_MAX_AGE)
        .unwrap_or(false)
}

fn fetch_and_cache(cache_path: &PathBuf, sig_cache_path: &PathBuf) -> Result<Index> {
    let bytes = fetch_bytes(PRIMARY_URL)?;
    let sig_text = fetch_text(SIG_URL).context(
        "fetching index.json.minisig — the index must be signed before it can be trusted",
    )?;
    verify_index_signature(&bytes, &sig_text)
        .context("freshly fetched index.json failed signature verification")?;

    if let Some(dir) = cache_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(cache_path, &bytes)?;
    std::fs::write(sig_cache_path, &sig_text)?;
    serde_json::from_slice(&bytes).context("parsing index.json")
}

fn sig_cache_path(cache_path: &Path) -> PathBuf {
    let mut name = cache_path.file_name().unwrap_or_default().to_os_string();
    name.push(".minisig");
    cache_path.with_file_name(name)
}

fn fetch_text(url: &str) -> Result<String> {
    ureq::get(url)
        .call()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .into_string()
        .context("reading response body")
}

pub fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("~/.cache"))
        .join("bakery/index.json")
}

/// Download a binary blob from `primary_url`, falling back to `fallback_url`
/// on any network error. Returns the raw bytes.
pub fn fetch_binary(primary_url: &str, fallback_url: &str) -> Result<Vec<u8>> {
    match fetch_bytes(primary_url) {
        Ok(bytes) => Ok(bytes),
        Err(primary_err) => {
            eprintln!(
                "  primary URL failed ({}), trying GitHub fallback…",
                primary_err
            );
            fetch_bytes(fallback_url).context("both primary and GitHub fallback failed")
        }
    }
}

fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    let resp = ureq::get(url)
        .call()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let status = resp.status();
    if status != 200 {
        bail!("HTTP {status} from {url}");
    }
    let mut buf = Vec::new();
    resp.into_reader()
        .read_to_end(&mut buf)
        .context("reading response")?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway test-only minisign keypair, generated solely to produce
    // these fixtures (`minisign -G` then `minisign -S`). It has no
    // relationship to the real bakery signing key (PUBKEY above) and the
    // matching secret key was discarded — these are just fixed vectors to
    // exercise the verification code path deterministically.
    const TEST_PUBKEY: &str = "RWQTYQi9Fe4trQDQmbb9txWDxzUIPYs57J//A5wG9BHcZXgC8YP0Cf59";
    const TEST_DATA: &[u8] = b"{\"hello\":\"world\"}\n";
    const TEST_SIG: &str = "untrusted comment: signature from minisign secret key\n\
RUQTYQi9Fe4trXY/WBxk++476WhTqtVd3hlNWQj5h5DF8keP8sEJn22LDG2hloNgJesXt6HsTQs9uktayRVp/HB4XfC6e+rhYAs=\n\
trusted comment: timestamp:1784230084\tfile:test-data.json\thashed\n\
znmVfINB4jFDR2a4wuY8rOKlUBeSDOFjMkHYDXV3vxvAjK+r4V12ae9ZRQkfVtQ1YIEmFXbnJfbxywg+NR/1AA==\n";

    #[test]
    fn valid_signature_verifies() {
        verify_against_key(TEST_DATA, TEST_SIG, TEST_PUBKEY)
            .expect("known-good signature must verify");
    }

    #[test]
    fn tampered_bytes_fail_verification() {
        let tampered = b"{\"hello\":\"world!\"}\n".to_vec();
        assert!(verify_against_key(&tampered, TEST_SIG, TEST_PUBKEY).is_err());
    }

    #[test]
    fn wrong_key_fails_verification() {
        // PUBKEY is the real production key — unrelated to the throwaway
        // TEST_PUBKEY the fixture was signed with, so it must not verify.
        assert!(verify_against_key(TEST_DATA, TEST_SIG, PUBKEY).is_err());
    }

    #[test]
    fn malformed_signature_text_errors_cleanly() {
        assert!(verify_against_key(TEST_DATA, "not a real signature", TEST_PUBKEY).is_err());
    }

    #[test]
    fn production_pubkey_constant_is_well_formed() {
        // Guards against a future typo/truncation in the hardcoded PUBKEY —
        // it must at least parse as a valid minisign public key.
        PublicKey::from_base64(PUBKEY).expect("PUBKEY must be a valid minisign public key");
    }
}
