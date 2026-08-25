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

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_string(&self.counts) {
            let _ = fs::write(&self.path, json);
        }
    }

    /// In-memory history with no backing file — [`save`](Self::save) is a
    /// silent no-op (an empty `path`). Lets a test (or a future in-memory
    /// host) control counts directly instead of writing through
    /// `~/.cache/<app>/history.json`.
    #[cfg(test)]
    pub(crate) fn from_counts(counts: HashMap<String, u32>) -> Self {
        LaunchHistory {
            counts,
            path: PathBuf::new(),
        }
    }
}
