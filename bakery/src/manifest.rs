use crate::track::Track;
use anyhow::{bail, Context, Result};
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const DEFAULT_BASE_URL: &str = "https://dl.breadway.dev";
const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// The `https://dl.breadway.dev` base can be overridden for local/staging
/// testing (e.g. serving a fake index from `python3 -m http.server`) without
/// rebuilding bakery — same pattern as `main.rs`'s `BAKERY_BIN_DIR` override.
fn base_url() -> String {
    std::env::var("BAKERY_INDEX_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}

/// Index URL for `track`. `Stable` keeps the exact pre-track path
/// (`{base}/index.json`) so existing infra and warm caches are unaffected;
/// `Beta`/`Dev` live under a track-prefixed subpath.
fn primary_url(track: Track) -> String {
    match track {
        Track::Stable => format!("{}/index.json", base_url()),
        Track::Beta | Track::Dev => format!("{}/{}/index.json", base_url(), track.as_str()),
    }
}

fn sig_url(track: Track) -> String {
    format!("{}.minisig", primary_url(track))
}

/// The bakery index-signing public key.
///
/// The matching secret key is used offline (never on this machine, never in
/// this repo) to sign `index.json` with `minisign` as part of publishing a
/// new index — see `scripts/gen-index.sh`. Every fetch of `index.json`, and
/// every load of the on-disk cache, must verify against this key before the
/// bytes are trusted or parsed. This is the single control point: the
/// per-artifact `sha256` fields and `post_install` hook strings all live
/// inside `index.json` itself, so a valid signature transitively covers them.
const PUBKEY: &str = "RWTBR8w/IJ+jaylOv80b52DzekKbSR2CvOVGvzB0ipGBaMhJPAOiEWq8";

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
    let public_key = PublicKey::from_base64(pubkey_b64).context("public key is malformed")?;
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
    /// License artifact filename (e.g. "LICENSE"), installed to
    /// `~/.local/share/licenses/<name>/LICENSE` — the bakery equivalent of
    /// what a PKGBUILD's `package()` does with `/usr/share/licenses`.
    #[serde(default)]
    pub license_file: Option<String>,
    #[serde(default)]
    pub license_file_sha256: Option<String>,
    /// Desktop entry artifact filename (e.g. "breadhelp.desktop"),
    /// installed to `~/.local/share/applications/<name>.desktop` so the
    /// app shows up in any XDG-compliant launcher without root.
    #[serde(default)]
    pub desktop_file: Option<String>,
    #[serde(default)]
    pub desktop_file_sha256: Option<String>,
    /// Data archive artifact filename (e.g. "content.tar.gz") — a `.tar.gz`
    /// in the release dir, extracted to `~/.local/share/<name>/` on
    /// install. For arbitrary data a package needs at runtime beyond a
    /// config example (e.g. breadhelp's guide content), where a single
    /// downloadable file + `tar` extraction is simpler than teaching
    /// bakery to mirror a whole directory tree file-by-file.
    #[serde(default)]
    pub data_archive: Option<String>,
    #[serde(default)]
    pub data_archive_sha256: Option<String>,
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

/// Load the manifest for `track`, using the on-disk cache when it is fresh
/// enough. Always fetches if `force_refresh` is true.
///
/// Every path — fresh fetch or cached read — verifies the minisign
/// signature over the raw `index.json` bytes before the JSON is parsed or
/// trusted. A signature failure on a freshly fetched index is always a hard
/// error. A signature failure on the *cached* copy is treated as a
/// (possibly tampered, possibly just stale-format) cache and triggers one
/// re-fetch from the network rather than bricking the CLI outright; if the
/// freshly fetched copy also fails to verify, that's a hard error.
pub fn load(force_refresh: bool, track: Track) -> Result<Index> {
    let cache_path = cache_path(track);
    let sig_cache_path = sig_cache_path(&cache_path);

    if !force_refresh && cache_is_fresh(&cache_path) {
        match read_and_verify_cache(&cache_path, &sig_cache_path, track) {
            Ok(index) => return Ok(index),
            Err(err) => {
                eprintln!("  warning: cached index.json failed verification ({err}), re-fetching…");
            }
        }
    }

    match fetch_and_cache(&cache_path, &sig_cache_path, track) {
        Ok(index) => Ok(index),
        Err(fetch_err) => {
            // A network error shouldn't be a hard failure when a valid
            // signed cache is sitting right there on disk, even if it's
            // stale (or freshness was never checked because force_refresh
            // was set) — fall back to it rather than bricking the CLI.
            match read_and_verify_cache(&cache_path, &sig_cache_path, track) {
                Ok(index) => {
                    eprintln!(
                        "  warning: could not refresh {track} index ({fetch_err}) — \
                         using possibly-stale cached index"
                    );
                    Ok(index)
                }
                Err(_) => Err(fetch_err),
            }
        }
    }
}

fn read_and_verify_cache(cache_path: &Path, sig_cache_path: &Path, track: Track) -> Result<Index> {
    let bytes = std::fs::read(cache_path).context("reading cached index")?;
    let sig_text = std::fs::read_to_string(sig_cache_path)
        .context("reading cached index.json.minisig (cache predates signing support)")?;
    verify_index_signature(&bytes, &sig_text)
        .with_context(|| format!("cached {track} index failed signature verification"))?;
    serde_json::from_slice(&bytes).context("parsing cached index")
}

fn cache_is_fresh(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| SystemTime::now().duration_since(t).unwrap_or(CACHE_MAX_AGE) < CACHE_MAX_AGE)
        .unwrap_or(false)
}

fn fetch_and_cache(cache_path: &Path, sig_cache_path: &Path, track: Track) -> Result<Index> {
    let bytes = fetch_bytes(&primary_url(track)).with_context(|| {
        format!(
            "fetching {track} index — has a {track} build been published yet? \
             run 'bakery track set stable' to switch back"
        )
    })?;
    let sig_text = fetch_text(&sig_url(track)).context(
        "fetching index.json.minisig — the index must be signed before it can be trusted",
    )?;
    verify_index_signature(&bytes, &sig_text)
        .with_context(|| format!("freshly fetched {track} index failed signature verification"))?;

    bread_utils::atomic::write_atomic_bytes(cache_path, &bytes, None)
        .with_context(|| format!("writing cached {track} index"))?;
    bread_utils::atomic::write_atomic_bytes(sig_cache_path, sig_text.as_bytes(), None)
        .with_context(|| format!("writing cached {track} index signature"))?;
    serde_json::from_slice(&bytes).context("parsing index.json")
}

fn sig_cache_path(cache_path: &Path) -> PathBuf {
    let mut name = cache_path.file_name().unwrap_or_default().to_os_string();
    name.push(".minisig");
    cache_path.with_file_name(name)
}

fn fetch_text(url: &str) -> Result<String> {
    let bytes = fetch_bytes(url)?;
    String::from_utf8(bytes).context("response is not valid UTF-8")
}

/// Cache filename for `track`. `Stable` keeps the pre-track filename
/// (`index.json`) so an existing warm cache survives an upgrade to a
/// track-aware bakery; `Beta`/`Dev` get their own sibling files so switching
/// tracks doesn't clobber each other's cache.
pub fn cache_path(track: Track) -> PathBuf {
    let file_name = match track {
        Track::Stable => "index.json".to_string(),
        Track::Beta | Track::Dev => format!("index-{}.json", track.as_str()),
    };
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("~/.cache"))
        .join("bakery")
        .join(file_name)
}

/// Download a binary blob from `primary_url`, falling back to `fallback_url`
/// on any network error. Returns the raw bytes.
pub fn fetch_binary(primary_url: &str, fallback_url: &str) -> Result<Vec<u8>> {
    match fetch_bytes(primary_url) {
        Ok(bytes) => Ok(bytes),
        Err(primary_err) => {
            eprintln!(
                "  {}",
                crate::ui::note(&format!(
                    "primary URL failed ({primary_err}), trying GitHub fallback…"
                ))
            );
            fetch_bytes(fallback_url).context("both primary and GitHub fallback failed")
        }
    }
}

/// Comfortably above any real bakery artifact — caps how much of a response
/// gets buffered into memory before any trust check runs on it.
const MAX_RESPONSE_BYTES: u64 = 256 * 1024 * 1024;

/// How often (at most) the `\r`-overwritten progress line refreshes — a
/// LAN-speed download can push way more than one chunk per 100ms, and
/// printing on every chunk would flood the terminal instead of reassuring it.
const PROGRESS_THROTTLE: Duration = Duration::from_millis(100);
const CHUNK_SIZE: usize = 64 * 1024;

fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    use std::io::{IsTerminal, Read};
    let resp = ureq::get(url).call().map_err(|e| anyhow::anyhow!("{e}"))?;
    let status = resp.status();
    if status != 200 {
        bail!("HTTP {status} from {url}");
    }

    // Progress feedback only when there's a Content-Length to show progress
    // against and stderr is an actual terminal — a multi-MB binary with no
    // feedback at all looks like a hang, but piped/CI output shouldn't get
    // `\r` noise. A manual chunked read loop (instead of one `read_to_end`)
    // is what makes printing partway through the download possible, without
    // pulling in a progress-bar crate for what's meant to just be reassurance.
    let content_length: Option<u64> = resp.header("Content-Length").and_then(|v| v.parse().ok());
    // Progress is reassurance for multi-MB binaries. A 4 KB index fetch
    // drawing a 100% / 0.0 MB bar is noise, not feedback.
    const MIN_PROGRESS_BYTES: u64 = 256 * 1024;
    let show_progress =
        content_length.is_some_and(|n| n >= MIN_PROGRESS_BYTES) && std::io::stderr().is_terminal();

    let mut buf = Vec::new();
    let mut reader = resp.into_reader();
    let mut chunk = [0u8; CHUNK_SIZE];
    let mut last_print = std::time::Instant::now();
    loop {
        let n = reader.read(&mut chunk).context("reading response")?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() as u64 > MAX_RESPONSE_BYTES {
            bail!("response from {url} exceeds the {MAX_RESPONSE_BYTES}-byte limit");
        }
        if show_progress && last_print.elapsed() >= PROGRESS_THROTTLE {
            crate::ui::print_progress(buf.len() as u64, content_length.unwrap());
            last_print = std::time::Instant::now();
        }
    }
    if show_progress {
        crate::ui::print_progress(buf.len() as u64, content_length.unwrap());
        crate::ui::finish_progress();
    }
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

    #[test]
    fn stable_cache_path_matches_pre_track_filename() {
        // Must stay exactly "index.json" so an existing warm cache from a
        // pre-track bakery binary is still used after an upgrade.
        assert_eq!(cache_path(Track::Stable).file_name().unwrap(), "index.json");
    }

    #[test]
    fn beta_and_dev_cache_paths_are_distinct_siblings() {
        let stable = cache_path(Track::Stable);
        let beta = cache_path(Track::Beta);
        let dev = cache_path(Track::Dev);
        assert_ne!(stable, beta);
        assert_ne!(stable, dev);
        assert_ne!(beta, dev);
        assert_eq!(beta.parent(), stable.parent());
        assert_eq!(dev.parent(), stable.parent());
    }

    #[test]
    fn stable_url_has_no_track_prefix() {
        assert_eq!(
            primary_url(Track::Stable),
            format!("{}/index.json", base_url())
        );
    }

    #[test]
    fn beta_and_dev_urls_are_track_prefixed() {
        assert_eq!(
            primary_url(Track::Beta),
            format!("{}/beta/index.json", base_url())
        );
        assert_eq!(
            primary_url(Track::Dev),
            format!("{}/dev/index.json", base_url())
        );
    }

    fn minimal_package_json() -> &'static str {
        r#"{
            "name": "breadhelp",
            "description": "test",
            "version": "1.0.0",
            "binaries": [],
            "config": null
        }"#
    }

    #[test]
    fn license_and_desktop_fields_default_to_none_on_old_shape_json() {
        // Simulates an index.json produced before license_file/desktop_file
        // existed — must not fail to parse.
        let pkg: Package = serde_json::from_str(minimal_package_json()).unwrap();
        assert!(pkg.license_file.is_none());
        assert!(pkg.license_file_sha256.is_none());
        assert!(pkg.desktop_file.is_none());
        assert!(pkg.desktop_file_sha256.is_none());
    }

    #[test]
    fn license_and_desktop_fields_roundtrip() {
        let mut pkg: Package = serde_json::from_str(minimal_package_json()).unwrap();
        pkg.license_file = Some("LICENSE".to_string());
        pkg.license_file_sha256 = Some("abc123".to_string());
        pkg.desktop_file = Some("breadhelp.desktop".to_string());
        pkg.desktop_file_sha256 = Some("def456".to_string());

        let json = serde_json::to_string(&pkg).unwrap();
        let restored: Package = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.license_file.as_deref(), Some("LICENSE"));
        assert_eq!(restored.license_file_sha256.as_deref(), Some("abc123"));
        assert_eq!(restored.desktop_file.as_deref(), Some("breadhelp.desktop"));
        assert_eq!(restored.desktop_file_sha256.as_deref(), Some("def456"));
    }
}
