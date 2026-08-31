use std::{
    env,
    path::Path,
    process::{Command, Stdio},
};

use bread_utils::bread_client::BreadClient;

use crate::desktop::DesktopEntry;

fn pick_terminal() -> String {
    if let Ok(t) = env::var("TERMINAL") {
        if !t.is_empty() {
            return t;
        }
    }
    let path_var = env::var("PATH").unwrap_or_default();
    for t in ["foot", "kitty", "alacritty", "wezterm", "ghostty", "xterm"] {
        if path_var.split(':').any(|d| Path::new(d).join(t).exists()) {
            return t.to_string();
        }
    }
    "xterm".to_string()
}

/// Spawns `entry`'s command (through a terminal if `entry.terminal` is set)
/// and, on a successful spawn, publishes `event` via [`emit_launched`].
///
/// `app_id` here is the caller's **bread event-namespace id** (e.g.
/// `"box"` for breadbox) — NOT [`crate::LAUNCHER_APP`] (`"breadbox"`).
/// Those are two different identities that happen to look similar:
/// `LAUNCHER_APP` only picks the shared cache/history directory (see its own
/// doc comment), while `app_id` here is threaded straight into
/// `BreadClient::connect(app_id)` and must be the caller's own namespace, or
/// `BreadClient::emit`'s `validate_app_namespace` check
/// (`event.starts_with("bread.{app_id}.")`) rejects `event` and drops it
/// with only an eprintln — passing `LAUNCHER_APP` here by mistake is exactly
/// that bug. breadbox passes its own `"box"` (see breadbox's `APP_ID`) so
/// its events publish as `bread.box.*`, matching [`emit_launched`]'s doc
/// example below.
pub fn do_launch(entry: &DesktopEntry, app_id: &str, event: &str) {
    let cmd = entry.exec.trim();
    let spawned = if entry.terminal {
        let term = pick_terminal();
        Command::new(&term)
            .args(["-e", "bash", "-c", cmd])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    } else {
        Command::new("bash")
            .args(["-c", cmd])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    };
    if spawned.is_ok() {
        emit_launched(entry, app_id, event);
    }
}

/// Publishes `event` under `app_id`'s bread namespace after a successful
/// spawn — e.g. breadbox calls this with `app_id = "box"` and
/// `event = "bread.box.launched"`, its own namespace. Fire-and-forget and
/// non-fatal (`BreadClient::emit` never blocks or errors this caller) —
/// breadd being absent must never affect launching itself. `app_id` must be
/// the caller's *own* namespace id, not [`crate::LAUNCHER_APP`] — see
/// [`do_launch`]'s doc comment for why those are different identities and
/// what happens if they're confused.
pub fn emit_launched(entry: &DesktopEntry, app_id: &str, event: &str) {
    let id = if entry.id.is_empty() {
        entry.exec.as_str()
    } else {
        entry.id.as_str()
    };
    BreadClient::connect(app_id).emit(
        event,
        serde_json::json!({ "id": id, "name": entry.name }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors `bread_shared::apps::validate_app_namespace` exactly
    /// (`event.starts_with(&format!("bread.{app}."))`) without pulling in
    /// that crate here — this is the one check that decides whether
    /// [`emit_launched`]'s event actually gets published.
    fn passes_namespace_check(app_id: &str, event: &str) -> bool {
        event.starts_with(&format!("bread.{app_id}."))
    }

    #[test]
    fn documented_app_id_and_event_pair_passes_the_namespace_check() {
        // breadbox's real call site (breadbox/breadbox/src/main.rs):
        // APP_ID = "box", LAUNCHED_EVENT = "bread.box.launched".
        assert!(
            passes_namespace_check("box", "bread.box.launched"),
            "do_launch/emit_launched's own doc example must actually pass \
             BreadClient::emit's namespace check"
        );
    }

    #[test]
    fn launcher_app_is_not_a_valid_app_id_for_the_documented_event() {
        // The historical bug this doc fix guards against: passing
        // `LAUNCHER_APP` ("breadbox", the cache/history identity) as
        // `app_id` instead of the caller's own namespace id ("box") would
        // silently drop `bread.box.launched` — event.starts_with(
        // "bread.breadbox.") is false for "bread.box.launched".
        assert!(
            !passes_namespace_check(crate::LAUNCHER_APP, "bread.box.launched"),
            "LAUNCHER_APP must NOT satisfy the namespace check for the \
             documented event — if this ever passes, do_launch's doc comment \
             warning about confusing the two identities is wrong"
        );
    }

    #[test]
    fn emit_launched_does_not_panic_when_breadd_is_unreachable() {
        // No daemon is running in a test environment — emit_launched (and
        // the BreadClient::emit it wraps) must degrade silently rather than
        // panicking or blocking, for both a valid and a namespace-violating
        // app_id.
        let entry = DesktopEntry {
            id: "firefox.desktop".to_string(),
            name: "Firefox".to_string(),
            exec: "firefox".to_string(),
            icon_name: String::new(),
            icon_path: None,
            categories: vec![],
            wm_class: None,
            terminal: false,
        };
        emit_launched(&entry, "box", "bread.box.launched");
        emit_launched(&entry, crate::LAUNCHER_APP, "bread.box.launched");
    }
}
