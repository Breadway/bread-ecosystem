//! Generates `~/.config/hypr/layerrules.json` from the active shell theme's
//! `[compositor]` table (`THEME_SYSTEM_PLAN.md` §9). `scripts/ui/rules.lua`
//! reads this file and emits `hl.layer_rule` calls from it, keeping its own
//! hardcoded rules as a pcall-guarded fallback for when this file is missing
//! or malformed — so generation here never has to be perfect, only present.
//!
//! Scope (plan §9's appearance/placement boundary): a theme's `[compositor]`
//! table owns per-namespace *appearance* only — blur, ignore_alpha,
//! blur_popups, animation, no_anim (the [`crate::shell::LayerRule`] field
//! set). It never owns placement, workspace-assignment, or focus rules —
//! those stay Lua-side user policy (`rules.lua`'s `hl.window_rule` calls)
//! that this generator does not touch.

use std::path::PathBuf;

use crate::shell::ShellTheme;

fn config_home() -> PathBuf {
    if let Ok(v) = std::env::var("XDG_CONFIG_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
}

/// `~/.config/hypr/layerrules.json` (or under `$XDG_CONFIG_HOME` if set) —
/// alongside `binds.json`, `settings.json`, `monitors.json`, and
/// `autostart.json`, the established flat-JSON-under-`hypr/` convention
/// those files already use (see `~/.config/hypr/scripts/input/binds.lua` for
/// the read side of that pattern, which `scripts/ui/rules.lua` now mirrors).
pub fn layerrules_path() -> PathBuf {
    config_home().join("hypr").join("layerrules.json")
}

/// Render `theme`'s `[compositor]` table as the JSON object
/// `scripts/ui/rules.lua` expects: keyed by layer-shell namespace (e.g.
/// `"breadbar"`, `"breadbox"`), each value the namespace's
/// [`crate::shell::LayerRule`] fields. `compositor_rules()` returns a
/// `BTreeMap`, so namespace order is stable (alphabetical) across runs and a
/// rewritten file diffs cleanly.
pub fn layerrules_json(theme: &ShellTheme) -> String {
    serde_json::to_string_pretty(theme.compositor_rules())
        .expect("LayerRule serialization is infallible (no maps/floats that can fail)")
}

/// [`layerrules_json`] + atomic write (tmp + rename), the same durability
/// [`crate::write_shared_css_from`] uses so a reload can never observe a
/// half-written file.
pub fn write_layerrules(theme: &ShellTheme) -> std::io::Result<PathBuf> {
    let path = layerrules_path();
    let json = layerrules_json(theme);
    crate::output::atomic_write(&path, &json)?;
    Ok(path)
}

/// [`write_layerrules`] from the active theme ([`crate::shell::load`], which
/// never fails — a broken active theme falls back to the builtin). Used by
/// the `bread-theme layerrules` CLI subcommand.
pub fn write_layerrules_active() -> std::io::Result<PathBuf> {
    write_layerrules(&crate::shell::load())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn lock_xdg() -> std::sync::MutexGuard<'static, ()> {
        // Shared with `shell::tests::isolated_xdg`, which also mutates
        // XDG_CONFIG_HOME — must be the *same* lock, not a look-alike one,
        // or the two modules' parallel tests race each other's env var
        // reads (see `crate::test_support`'s doc comment).
        crate::test_support::XDG_CONFIG_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn with_config_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _lock = lock_xdg();
        let dir = std::env::temp_dir().join(format!(
            "bread-theme-layerrules-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&dir)));
        match old {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        match result {
            Ok(v) => v,
            Err(e) => std::panic::resume_unwind(e),
        }
    }

    #[test]
    fn layerrules_path_sits_under_config_hypr() {
        with_config_home(|dir| {
            assert_eq!(layerrules_path(), dir.join("hypr").join("layerrules.json"));
        });
    }

    /// `load_named("liquid-motion")` resolves through discovery (user dir,
    /// then system dir, then the compiled-in builtin) — isolate
    /// `XDG_CONFIG_HOME` to an empty dir so this can't pick up a real
    /// `~/.config/bread/themes/liquid-motion/theme.toml` override and land
    /// on a different `[compositor]` table than the builtin's.
    fn builtin_theme() -> ShellTheme {
        with_config_home(|_| crate::shell::load_named("liquid-motion").unwrap())
    }

    #[test]
    fn layerrules_json_covers_all_six_builtin_namespaces() {
        let theme = builtin_theme();
        let json = layerrules_json(&theme);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let obj = value.as_object().expect("top-level object");
        for ns in [
            "breadbar",
            "breadbar-osd",
            "breadbar-notif",
            "breadbar-panel",
            "breadbar-dismiss",
            "breadbox",
        ] {
            assert!(obj.contains_key(ns), "missing namespace {ns} in JSON");
        }
    }

    #[test]
    fn layerrules_json_shape_matches_breadbar_rule() {
        let theme = builtin_theme();
        let json = layerrules_json(&theme);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let bar = &value["breadbar"];
        assert_eq!(bar["blur"], true);
        assert_eq!(bar["ignore_alpha"], 0.2);
        assert_eq!(bar["blur_popups"], true);
        assert_eq!(bar["animation"], "slide top");
        // no_anim is a plain bool (not Option), so it's always present, even
        // when false — unlike ignore_alpha/animation which are omitted.
        assert_eq!(bar["no_anim"], false);

        let dismiss = &value["breadbar-dismiss"];
        assert_eq!(dismiss["no_anim"], true);
        // ignore_alpha/animation are unset for breadbar-dismiss, so the
        // skip_serializing_if omits them entirely rather than writing null.
        assert!(dismiss.get("ignore_alpha").is_none());
        assert!(dismiss.get("animation").is_none());
    }

    #[test]
    fn write_layerrules_active_writes_atomically_and_is_reloadable() {
        with_config_home(|dir| {
            let path = write_layerrules_active().unwrap();
            assert_eq!(path, dir.join("hypr").join("layerrules.json"));
            assert!(path.is_file());
            // No leftover .tmp file after the atomic rename.
            assert!(!path.with_file_name("layerrules.json.tmp").exists());

            let contents = std::fs::read_to_string(&path).unwrap();
            let value: serde_json::Value = serde_json::from_str(&contents).unwrap();
            assert!(value.as_object().unwrap().contains_key("breadbox"));

            // Rewriting (theme switch, pywal hook, etc.) must not fail or
            // leave a stale temp file behind.
            let path2 = write_layerrules_active().unwrap();
            assert_eq!(path, path2);
            assert!(!path.with_file_name("layerrules.json.tmp").exists());
        });
    }
}
