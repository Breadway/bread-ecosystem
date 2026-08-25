use std::{collections::HashMap, fs, path::PathBuf};

pub struct LaunchHistory {
    counts: HashMap<String, u32>,
    path: PathBuf,
}

impl LaunchHistory {
    /// `app` picks the cache subdirectory (see [`crate::cache_dir`]) the
    /// history file lives in.
    pub fn load(app: &str) -> Self {
        let path = crate::paths::cache_dir(app).join("history.json");
        let counts = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        LaunchHistory { counts, path }
    }

    pub fn count(&self, name: &str) -> u32 {
        self.counts.get(name).copied().unwrap_or(0)
    }

    pub fn increment(&mut self, name: &str) {
        *self.counts.entry(name.to_string()).or_insert(0) += 1;
    }

    /// Writes `counts` to `path` as JSON. Best-effort — a broken cache dir
    /// (missing parent, full disk, permissions) must not stop the caller
    /// from launching anything, so this never returns an error — but it now
    /// logs one on failure rather than swallowing it silently. Shared by two
    /// hosts (breadbox's overlay and breadbar's embedded capsule, both keyed
    /// under [`crate::LAUNCHER_APP`]), so a save failure here silently stops
    /// ranking history for both.
    pub fn save(&self) {
        match serde_json::to_string(&self.counts) {
            Ok(json) => {
                if let Err(err) = fs::write(&self.path, json) {
                    eprintln!(
                        "bread-launcher: failed to save launch history to {}: {err}",
                        self.path.display()
                    );
                }
            }
            Err(err) => {
                eprintln!("bread-launcher: failed to serialize launch history: {err}");
            }
        }
    }

    /// In-memory history with no backing file — [`save`](Self::save) fails
    /// (an empty `path` is not writable) and now logs that failure to
    /// stderr rather than swallowing it, same as any other broken-path
    /// case. Lets a test (or a future in-memory host) control counts
    /// directly instead of writing through `~/.cache/<app>/history.json`.
    #[cfg(test)]
    pub(crate) fn from_counts(counts: HashMap<String, u32>) -> Self {
        LaunchHistory {
            counts,
            path: PathBuf::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_history_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bread-launcher-history-test-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("history.json")
    }

    #[test]
    fn save_then_load_round_trips_counts() {
        let path = temp_history_path("roundtrip");
        let mut history = LaunchHistory {
            counts: HashMap::new(),
            path: path.clone(),
        };
        history.increment("firefox.desktop");
        history.increment("firefox.desktop");
        history.increment("kitty.desktop");
        history.save();

        let text = std::fs::read_to_string(&path).expect("save should have written the file");
        let counts: HashMap<String, u32> = serde_json::from_str(&text).unwrap();
        assert_eq!(counts.get("firefox.desktop"), Some(&2));
        assert_eq!(counts.get("kitty.desktop"), Some(&1));

        // load() from the same path should see the same counts.
        let reloaded = LaunchHistory {
            counts: serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap(),
            path: path.clone(),
        };
        assert_eq!(reloaded.count("firefox.desktop"), 2);

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// `save()` on an unwritable path (e.g. the parent directory doesn't
    /// exist, or `path` is empty) must not panic — it's best-effort, called
    /// from a launcher's shutdown path where a hard failure would be worse
    /// than a lost history entry. This exercises exactly the failure branch
    /// the `eprintln!` above was added for; there is no return value to
    /// assert on, so "did not panic" is the contract under test.
    #[test]
    fn save_to_a_broken_path_does_not_panic() {
        let history = LaunchHistory::from_counts(HashMap::from([("x".to_string(), 1)]));
        history.save();

        let history = LaunchHistory {
            counts: HashMap::new(),
            path: PathBuf::from("/nonexistent-dir/definitely-not-there/history.json"),
        };
        history.save();
    }
}
