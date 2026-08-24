use std::{env, path::PathBuf};

// ---- XDG path helpers -------------------------------------------------------

pub fn home_dir() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
}

/// `$XDG_CACHE_HOME/<app>` (or `~/.cache/<app>`). `app` is the caller's own
/// name in this scheme — e.g. breadbox passes `"breadbox"` to keep using the
/// on-disk layout it always has; a future host picks its own.
pub fn cache_dir(app: &str) -> PathBuf {
    env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".cache"))
        .join(app)
}

/// `$XDG_CONFIG_HOME/<app>` (or `~/.config/<app>`). See [`cache_dir`].
pub fn config_dir(app: &str) -> PathBuf {
    env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".config"))
        .join(app)
}

/// The `applications/` directories a `.desktop` file may live in, per the
/// XDG base-directory spec (system-wide first, user-local last so later
/// entries can override earlier ones on lookup by filename).
pub fn app_dirs() -> Vec<PathBuf> {
    let home = home_dir();
    let mut dirs = vec![PathBuf::from("/usr/share/applications")];

    let xdg_data_dirs =
        env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in xdg_data_dirs.split(':') {
        let p = PathBuf::from(d).join("applications");
        if p != dirs[0] {
            dirs.push(p);
        }
    }

    dirs.push(
        env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".local/share"))
            .join("applications"),
    );
    dirs
}
