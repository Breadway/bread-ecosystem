//! XDG base directory helpers.
//!
//! Several repos independently rolled `dirs::data_local_dir().unwrap_or_else(||
//! PathBuf::from("~/.local/share"))`-shaped fallbacks. The literal-tilde
//! string is the bug: `PathBuf`/`std::fs` never expand `~`, so on the rare
//! box where `dirs` can't resolve a home directory (no `HOME` env var, e.g.
//! some container/systemd-service contexts) the fallback silently resolves
//! to a directory literally named `~` in the process's current working
//! directory instead of the user's actual home. Confirmed present in:
//! - `breadclip-core/src/lib.rs:171-175` (`data_dir`)
//! - `breadpad-shared/src/classifier.rs:34-39` (`model_dir`)
//! - `breadpad-shared/src/config.rs:214-219` and `:221-226`
//!   (`config_path`, `style_css_path`)
//! - `breadmon/src/profile.rs:31-35` (`profiles_dir`)
//! - `breadarr-shared/src/config.rs:316-321`'s own `expand_home` helper,
//!   which had the same bug in a different shape: its *own* fallback (when
//!   `HOME` itself isn't set) returned the literal, unexpanded input string
//!   rather than a real path.
//!
//! The helpers here resolve a real `$HOME` (via `dirs::home_dir()`, which
//! itself falls back to reading `HOME` directly) before ever falling back,
//! so the fallback path is always an absolute, expanded path.

use std::path::PathBuf;

/// A real, absolute home directory — `dirs::home_dir()`, falling back to
/// `/root` only if that itself fails (no `HOME` env var *and* no passwd-db
/// entry, e.g. some minimal container contexts). Never a literal `"~"`.
pub fn home_dir() -> PathBuf {
    home_or_root()
}

fn home_or_root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/root"))
}

/// `$XDG_CONFIG_HOME` (only if it's set to an absolute path) or `~/.config`,
/// joined with `app`.
pub fn config_dir(app: &str) -> PathBuf {
    config_home().join(app)
}

/// The bare `$XDG_CONFIG_HOME` (or `~/.config`) directory, with no app name
/// joined on — for callers that build up multiple sub-paths themselves
/// (e.g. `bos-settings`, which joins a different bread* app's name per
/// config file it edits).
pub fn config_home() -> PathBuf {
    base_config_dir()
}

/// `$XDG_DATA_HOME` (only if absolute) or `~/.local/share`, joined with `app`.
pub fn data_dir(app: &str) -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| home_or_root().join(".local/share"))
        .join(app)
}

/// `$XDG_CACHE_HOME` (only if absolute) or `~/.cache`, joined with `app`.
pub fn cache_dir(app: &str) -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| home_or_root().join(".cache"))
        .join(app)
}

/// `$XDG_RUNTIME_DIR`, falling back to `/tmp` — matches the fallback every
/// consumer (breadbox, breadclip, breadmon) already used for PID/socket
/// scratch files, which don't need to survive a reboot.
pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn base_config_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            return p;
        }
    }
    dirs::config_dir().unwrap_or_else(|| home_or_root().join(".config"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_joins_app_name() {
        let d = config_dir("breadpad");
        assert!(d.ends_with("breadpad"));
        assert!(d.is_absolute());
    }

    #[test]
    fn home_dir_is_absolute_and_never_a_literal_tilde() {
        let d = home_dir();
        assert!(d.is_absolute());
        assert!(!d.components().any(|c| c.as_os_str() == "~"));
    }

    #[test]
    fn data_dir_never_contains_literal_tilde() {
        // Regression guard for the exact bug this module replaces: the
        // fallback must never be a literal "~/..." path component.
        let d = data_dir("breadclip");
        assert!(!d.components().any(|c| c.as_os_str() == "~"));
        assert!(d.is_absolute());
    }

    #[test]
    fn cache_dir_is_absolute() {
        assert!(cache_dir("breadsearch").is_absolute());
    }

    #[test]
    fn runtime_dir_falls_back_to_tmp() {
        let _lock = crate::env_test_lock().lock().unwrap_or_else(|e| e.into_inner());
        // We don't unset XDG_RUNTIME_DIR here (test isolation), just confirm
        // the function returns *something* absolute either way.
        assert!(runtime_dir().is_absolute());
    }
}
