use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
    pub binaries: Vec<String>,
    pub services: Vec<String>,
    pub installed_at: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct State {
    pub packages: HashMap<String, InstalledPackage>,
}

impl State {
    pub fn load() -> Result<Self> {
        let path = state_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).context("reading installed.json")?;
        serde_json::from_str(&text).context("parsing installed.json")
    }

    pub fn save(&self) -> Result<()> {
        let path = state_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        // Write to a temp file then rename for atomicity — avoids a torn write
        // if the process is killed mid-save.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &text).context("writing installed.json.tmp")?;
        std::fs::rename(&tmp, &path).context("atomically replacing installed.json")?;
        Ok(())
    }

    pub fn is_installed(&self, name: &str) -> bool {
        self.packages.contains_key(name)
    }

    pub fn record(&mut self, pkg: InstalledPackage) {
        self.packages.insert(pkg.name.clone(), pkg);
    }

    pub fn remove(&mut self, name: &str) -> Option<InstalledPackage> {
        self.packages.remove(name)
    }
}

fn state_path() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("~"))
                .join(".local/state")
        })
        .join("bakery/installed.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str, version: &str) -> InstalledPackage {
        InstalledPackage {
            name: name.to_string(),
            version: version.to_string(),
            binaries: vec![],
            services: vec![],
            installed_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn record_and_is_installed() {
        let mut state = State::default();
        assert!(!state.is_installed("foo"));
        state.record(pkg("foo", "1.0.0"));
        assert!(state.is_installed("foo"));
    }

    #[test]
    fn remove_installed() {
        let mut state = State::default();
        state.record(pkg("foo", "1.0.0"));
        let removed = state.remove("foo");
        assert!(removed.is_some());
        assert!(!state.is_installed("foo"));
    }

    #[test]
    fn remove_unknown_returns_none() {
        let mut state = State::default();
        assert!(state.remove("nope").is_none());
    }

    #[test]
    fn json_roundtrip() {
        let mut state = State::default();
        state.record(InstalledPackage {
            name: "bar".to_string(),
            version: "2.0.0".to_string(),
            binaries: vec!["bar".to_string()],
            services: vec!["bar.service".to_string()],
            installed_at: "2026-06-01T00:00:00Z".to_string(),
        });
        let json = serde_json::to_string(&state).unwrap();
        let restored: State = serde_json::from_str(&json).unwrap();
        assert!(restored.is_installed("bar"));
        assert_eq!(restored.packages["bar"].version, "2.0.0");
        assert_eq!(restored.packages["bar"].services, ["bar.service"]);
    }
}
