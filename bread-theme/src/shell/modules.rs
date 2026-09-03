//! The set of bar-module names a `[bar.slots]` entry may reference.
//!
//! A slot entry is one of:
//! - a **built-in module name** ([`BUILTIN_MODULES`]) — a widget the shell app
//!   constructs itself;
//! - a name the app **registered at startup** ([`register`]) — how a new
//!   built-in module is added without editing this crate (plan §2 phase 3);
//! - a `widget:<key>` entry — a Lua-declared widget or a `WidgetPlacement`
//!   alias, validated only by its `widget:` prefix (the suffix is open);
//! - the `"+"` splice marker inside a theme that `extends` another (replaced
//!   at merge time — see `manifest::merge_values`).
//!
//! `list()` / the `bread` daemon load themes without an app, so they see only
//! [`BUILTIN_MODULES`] plus whatever any in-process [`register`] call added.

use std::sync::RwLock;

/// Modules every shell knows how to build. breadbar constructs each of these
/// in `main.rs` and registers it in `bar::slots::ModuleRegistry`.
pub const BUILTIN_MODULES: &[&str] = &[
    "workspaces",
    "media",
    "clock",
    "volume",
    "wifi",
    "battery",
    "control",
    "cpu",
    "ram",
    "launcher_entry",
    "launcher_results",
];

static REGISTERED: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Declare additional module names this process's shell app can place in a
/// slot. Idempotent; call once at startup, before loading the active theme.
/// Names already in [`BUILTIN_MODULES`] are ignored.
pub fn register<I, S>(names: I)
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut reg = REGISTERED.write().unwrap_or_else(|e| e.into_inner());
    for n in names {
        let n = n.into();
        if !BUILTIN_MODULES.contains(&n.as_str()) && !reg.contains(&n) {
            reg.push(n);
        }
    }
}

/// Every module name a slot entry may currently reference — built-ins plus
/// anything [`register`]ed. For a settings UI enumerating what a theme may
/// put in a slot (bos-settings, plan §5).
pub fn all() -> Vec<String> {
    let mut out: Vec<String> = BUILTIN_MODULES.iter().map(|s| s.to_string()).collect();
    out.extend(
        REGISTERED
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned(),
    );
    out
}

/// Whether `name` is a currently-valid slot-entry module (not counting the
/// `widget:` prefix or the `"+"` marker — the caller handles those).
pub(super) fn is_known(name: &str) -> bool {
    BUILTIN_MODULES.contains(&name)
        || REGISTERED
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|m| m == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_always_known() {
        for m in BUILTIN_MODULES {
            assert!(is_known(m));
        }
    }

    #[test]
    fn register_adds_new_names_and_ignores_builtins_and_dupes() {
        register(["git-branch", "cpu", "git-branch"]);
        assert!(is_known("git-branch"));
        let all = all();
        assert_eq!(all.iter().filter(|m| *m == "git-branch").count(), 1);
        // `cpu` is a builtin — not duplicated into the registered list.
        assert_eq!(all.iter().filter(|m| *m == "cpu").count(), 1);
    }
}
