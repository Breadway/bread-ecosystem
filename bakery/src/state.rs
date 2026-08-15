use crate::track::Track;
use anyhow::{Context, Result};
use fs4::FileExt;
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
    // `#[serde(default)]` so an installed.json written before per-package
    // track tracking existed still deserializes — defaults to Stable, same
    // convention as `State.track` above.
    #[serde(default)]
    pub track: Track,
    /// The version this package was upgraded from, if any — `bakery
    /// rollback` uses this to find the matching local backup dir. `None` on
    /// a fresh first-time install. `#[serde(default)]` for the same
    /// old-shape-json reason as `track` above.
    #[serde(default)]
    pub previous_version: Option<String>,
    /// SHA-256 (hex) of each installed binary, captured at install time.
    /// `bakery verify` recomputes these from disk and compares against this
    /// recorded value rather than a fresh index lookup — the index only
    /// carries the checksum for whatever the *current latest* release is,
    /// which may not match what's actually installed. Empty on installs
    /// that predate this field.
    #[serde(default)]
    pub binary_sha256: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct State {
    // `#[serde(default)]` lets an installed.json written by a pre-track
    // bakery binary deserialize straight into Track::Stable with no
    // migration step.
    #[serde(default)]
    pub track: Track,
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
        let text = serde_json::to_string_pretty(self)?;
        bread_utils::atomic::write_atomic(&path, &text, None).context("writing installed.json")
    }

    /// Runs `f` against a freshly-loaded `State` while holding an exclusive
    /// lock on a sibling `installed.json.lock` file, saving the result if `f`
    /// succeeds. Without this, two concurrent `bakery` invocations each
    /// load-mutate-save `installed.json` independently and the second save
    /// silently drops the first's change — the lock serializes the whole
    /// read-modify-write instead of just the final write.
    pub fn with_lock<T>(f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        let lock_path = PathBuf::from(format!("{}.lock", state_path().display()));
        if let Some(dir) = lock_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .context("opening installed.json.lock")?;
        lock_file
            .lock_exclusive()
            .context("locking installed.json.lock")?;

        let mut state = Self::load()?;
        let result = f(&mut state)?;
        state.save()?;
        // Lock releases when `lock_file` drops at end of scope.
        Ok(result)
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

    pub fn set_track(&mut self, track: Track) {
        self.track = track;
    }
}

fn state_base_dir() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(".local/state")
    })
}

fn state_path() -> PathBuf {
    bakery_state_dir().join("installed.json")
}

/// Per-user bakery state dir (`~/.local/state/bakery`). Independent of the
/// install prefix — system-prefix installs still record what this user asked for.
pub fn bakery_state_dir() -> PathBuf {
    state_base_dir().join("bakery")
}

/// Local backup dir for `pkg_name`'s `version` binaries, populated by
/// `install::install_package` right before an update overwrites the
/// previous binaries and consumed by `bakery rollback`. See
/// `install::backup_current_binary`'s doc comment for why this is a local
/// snapshot rather than a re-fetch of the old version from the server.
pub fn backup_dir(pkg_name: &str, version: &str) -> PathBuf {
    state_base_dir()
        .join("bakery/backups")
        .join(pkg_name)
        .join(version)
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
            track: Track::Stable,
            previous_version: None,
            binary_sha256: HashMap::new(),
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
    fn track_defaults_to_stable_on_old_shape_json() {
        // Simulates installed.json written before the track field existed.
        let old_shape = r#"{"packages":{}}"#;
        let state: State = serde_json::from_str(old_shape).unwrap();
        assert_eq!(state.track, Track::Stable);
    }

    #[test]
    fn set_track_updates_and_roundtrips() {
        let mut state = State::default();
        state.set_track(Track::Dev);
        let json = serde_json::to_string(&state).unwrap();
        let restored: State = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.track, Track::Dev);
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
            track: Track::Beta,
            previous_version: Some("1.0.0".to_string()),
            binary_sha256: HashMap::from([("bar".to_string(), "abc123".to_string())]),
        });
        let json = serde_json::to_string(&state).unwrap();
        let restored: State = serde_json::from_str(&json).unwrap();
        assert!(restored.is_installed("bar"));
        assert_eq!(restored.packages["bar"].version, "2.0.0");
        assert_eq!(restored.packages["bar"].services, ["bar.service"]);
        assert_eq!(restored.packages["bar"].track, Track::Beta);
        assert_eq!(
            restored.packages["bar"].previous_version.as_deref(),
            Some("1.0.0")
        );
        assert_eq!(restored.packages["bar"].binary_sha256["bar"], "abc123");
    }

    #[test]
    fn installed_package_track_defaults_to_stable_on_old_shape_json() {
        // Simulates an installed.json entry written before per-package track
        // tracking existed.
        let old_shape = r#"{"name":"foo","version":"1.0.0","binaries":[],"services":[],"installed_at":"2026-01-01T00:00:00Z"}"#;
        let installed: InstalledPackage = serde_json::from_str(old_shape).unwrap();
        assert_eq!(installed.track, Track::Stable);
    }

    #[test]
    fn installed_package_previous_version_and_binary_sha256_default_on_old_shape_json() {
        // Simulates an installed.json entry written before rollback/verify
        // support existed.
        let old_shape = r#"{"name":"foo","version":"1.0.0","binaries":[],"services":[],"installed_at":"2026-01-01T00:00:00Z","track":"stable"}"#;
        let installed: InstalledPackage = serde_json::from_str(old_shape).unwrap();
        assert!(installed.previous_version.is_none());
        assert!(installed.binary_sha256.is_empty());
    }

    #[test]
    fn bakery_state_dir_is_under_state_home_and_independent_of_prefix() {
        let dir = bakery_state_dir();
        assert!(dir.ends_with("bakery"));
        // Must not follow BAKERY_PREFIX — state is always per-user.
        assert!(!dir.starts_with("/usr/local"));
    }

    #[test]
    fn backup_dir_is_distinct_per_package_and_version() {
        let a = backup_dir("bakery", "0.3.1");
        let b = backup_dir("bakery", "0.3.2");
        let c = backup_dir("breadhelp", "0.3.1");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(a.ends_with("bakery/backups/bakery/0.3.1"));
    }

    #[test]
    fn with_lock_persists_mutation_across_reload() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY (test-only): temporarily redirects the state dir env var so
        // this test doesn't touch the real ~/.local/state/bakery/installed.json.
        std::env::set_var("XDG_STATE_HOME", dir.path());

        State::with_lock(|state| {
            state.record(pkg("foo", "1.0.0"));
            Ok(())
        })
        .unwrap();

        let reloaded = State::load().unwrap();
        assert!(reloaded.is_installed("foo"));

        std::env::remove_var("XDG_STATE_HOME");
    }
}
