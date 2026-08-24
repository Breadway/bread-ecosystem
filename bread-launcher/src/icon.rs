use std::{fs, path::PathBuf};

pub struct IconCache {
    pub dir: PathBuf,
}

impl IconCache {
    /// `app` picks the cache subdirectory (see [`crate::cache_dir`]) — pass
    /// the same name across a process's calls so `path_for` and
    /// `manifest_path` agree on where icons live.
    pub fn new(app: &str) -> Self {
        IconCache { dir: crate::paths::cache_dir(app).join("icons") }
    }

    pub fn path_for(&self, icon_name: &str) -> PathBuf {
        self.dir.join(format!("{}.png", icon_name))
    }

    pub fn manifest_path(app: &str) -> PathBuf {
        crate::paths::cache_dir(app).join("manifest.json")
    }

    pub fn ensure_dir(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)
    }
}
