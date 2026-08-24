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

/// Publishes `event` (e.g. `bread.box.launched`) as `app_id` after a
/// successful spawn. Fire-and-forget and non-fatal (`BreadClient::emit`
/// never blocks or errors this caller) — breadd being absent must never
/// affect launching itself.
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
