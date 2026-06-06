use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const PRIMARY_URL: &str = "https://dl.breadway.dev/index.json";
const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ConfigScaffold {
    pub dir: String,
    /// relative to the product repo root; copied as-is if absent at install time
    pub example: Option<String>,
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
    pub bread_deps: Vec<String>,
    #[serde(default)]
    pub services: Vec<Service>,
    pub config: Option<ConfigScaffold>,
    #[serde(default)]
    pub post_install: Vec<String>,
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
pub fn load(force_refresh: bool) -> Result<Index> {
    let cache_path = cache_path();

    if !force_refresh && cache_is_fresh(&cache_path) {
        let text = std::fs::read_to_string(&cache_path)
            .context("reading cached index")?;
        return serde_json::from_str(&text).context("parsing cached index");
    }

    fetch_and_cache(&cache_path)
}

fn cache_is_fresh(path: &PathBuf) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| SystemTime::now().duration_since(t).unwrap_or(CACHE_MAX_AGE) < CACHE_MAX_AGE)
        .unwrap_or(false)
}

fn fetch_and_cache(cache_path: &PathBuf) -> Result<Index> {
    let text = fetch_text(PRIMARY_URL)?;
    if let Some(dir) = cache_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(cache_path, &text)?;
    serde_json::from_str(&text).context("parsing index.json")
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
        .context("reading binary")?;
    Ok(buf)
}
