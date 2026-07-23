//! Runs each capture target inside a throwaway nested Hyprland instance
//! instead of the operator's live desktop, so nothing on their screen (other
//! windows, a differently-themed real bar, whatever's behind a popover) can
//! leak into a capture, and the capture in turn never flashes across their
//! desktop either.
//!
//! Mechanics, established empirically against Hyprland 0.55 (Lua config) —
//! there's no single documented "run headless" switch that fits this case:
//! - A genuinely headless (zero real output) backend needs
//!   `AQ_NO_KMS_REQUIREMENT`, but that's for a vGPU with no display output at
//!   all. This machine's GPU already has a real output claimed by the live
//!   session's `Hyprland`, and only one process can hold that session's seat
//!   (logind) at a time — a second instance trying DRM directly fails with
//!   "Device or resource busy", not a headless fallback.
//! - The working path is nesting: keep `WAYLAND_DISPLAY` pointed at the
//!   outer session so the new `Hyprland` connects to it as an ordinary
//!   Wayland *client* (Aquamarine's Wayland backend) — from the outer
//!   session's point of view it's just a window. It auto-picks its own new
//!   server socket name (`wayland-N`, skipping ones already taken) and its
//!   own `HYPRLAND_INSTANCE_SIGNATURE`; neither is knowable in advance, so
//!   both are discovered by diffing directory listings before/after spawn.
//! - That nested window's pixel size is decided by the *outer* compositor
//!   (it's just a regular window there), not by any monitor rule inside the
//!   nested config — so getting a fixed, consistent capture canvas means
//!   floating + exact-resizing it via one-shot `hyprctl --instance <outer>
//!   dispatch` calls against the outer session, targeted at the new
//!   window's `address` (found by matching the outer client list's `pid`
//!   against the spawned process's own pid — unambiguous, no reliance on
//!   window class/title/timing).
//! - The outer compositor throttles frame callbacks for occluded surfaces:
//!   with the nested window unfocused/covered, `grim` (run *inside* the
//!   nested session) hangs forever waiting on `ext_image_copy_capture`'s
//!   `.ready()` event, because Aquamarine's Wayland backend never gets a
//!   frame tick to render one. Focusing the nested window (raising it, so
//!   it's actually presented) is what makes captures complete instead of
//!   hanging — confirmed by reproducing the hang and then clearing it with
//!   nothing else changed.
//! - Vanilla Hyprland draws its own branded default background/logo when no
//!   client owns the background layer (not blank), plus a red on-screen
//!   watchdog/XDG-desktop/gui-utils warning overlay when started directly
//!   like this rather than via `start-hyprland`. Both are disabled via
//!   `misc:*` config, not by launch flags.

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const HYPRCTL_TIMEOUT: Duration = Duration::from_secs(3);
/// Settle time after focusing the nested window — gives the outer
/// compositor a moment to actually start presenting it (see module docs on
/// occlusion throttling) before anything tries to capture through it.
const FOCUS_SETTLE: Duration = Duration::from_millis(400);

pub struct Isolation {
    child: Child,
    pub wayland_display: String,
    pub instance_signature: String,
    config_path: PathBuf,
    runtime_dir: PathBuf,
}

impl Isolation {
    /// Spawn the nested instance, size its outer window to `width`x`height`,
    /// and set `WAYLAND_DISPLAY`/`HYPRLAND_INSTANCE_SIGNATURE` on *this*
    /// process's own environment so every subsequent
    /// `bread_utils::proc::run` spawn (the target app, and in turn its own
    /// `grim` calls) inherits them and lands inside the nested session.
    pub fn start(width: u32, height: u32) -> Result<Self> {
        let outer_wayland_display =
            std::env::var("WAYLAND_DISPLAY").context("WAYLAND_DISPLAY not set — isolation requires running inside a live Hyprland/Wayland session to nest inside")?;
        let outer_signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
            .context("HYPRLAND_INSTANCE_SIGNATURE not set — isolation requires running inside a live Hyprland session")?;
        let runtime_dir = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string()));

        let config_path = write_nested_config()?;
        let hypr_dir = runtime_dir.join("hypr");
        let before_instances = dir_names(&hypr_dir);
        let before_sockets: HashSet<String> = dir_names(&runtime_dir).into_iter().filter(|n| is_wayland_socket_name(n)).collect();

        let child = Command::new("Hyprland")
            .arg("--config")
            .arg(&config_path)
            .env("WAYLAND_DISPLAY", &outer_wayland_display)
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawning nested Hyprland")?;
        let pid = child.id();

        let mut isolation = Isolation {
            child,
            wayland_display: String::new(),
            instance_signature: String::new(),
            config_path,
            runtime_dir: runtime_dir.clone(),
        };

        let result = (|| -> Result<()> {
            isolation.instance_signature = poll_for_new(&hypr_dir, &before_instances, DISCOVERY_TIMEOUT, |_| true)
                .context("waiting for nested Hyprland's instance signature to appear")?;
            isolation.wayland_display = poll_for_new(&runtime_dir, &before_sockets, DISCOVERY_TIMEOUT, is_wayland_socket_name)
                .context("waiting for nested Hyprland's Wayland socket to appear")?;

            let addr = poll_for_client_address(&outer_signature, pid, DISCOVERY_TIMEOUT)
                .context("waiting for the nested Hyprland window to appear in the outer session")?;
            outer_dispatch(&outer_signature, &format!("hl.dsp.window.float({{ window = 'address:{addr}' }})"))?;
            outer_dispatch(
                &outer_signature,
                &format!("hl.dsp.window.resize({{ x = {width}, y = {height}, window = 'address:{addr}' }})"),
            )?;
            // Must be focused/raised, not just resized — an occluded nested
            // window never gets frame callbacks from the outer compositor,
            // and grim run inside it hangs forever waiting on one. See
            // module docs.
            outer_dispatch(&outer_signature, &format!("hl.dsp.focus({{ window = 'address:{addr}' }})"))?;
            std::thread::sleep(FOCUS_SETTLE);
            Ok(())
        })();

        if let Err(e) = result {
            // Best-effort teardown of the half-started instance before
            // propagating — the normal Drop impl still runs too, but doing
            // it here as well means a failure this early doesn't depend on
            // isolation ever being bound to a variable that outlives this
            // function.
            let _ = isolation.child.kill();
            let _ = isolation.child.wait();
            return Err(e);
        }

        std::env::set_var("WAYLAND_DISPLAY", &isolation.wayland_display);
        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", &isolation.instance_signature);

        Ok(isolation)
    }
}

impl Drop for Isolation {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.config_path);
        if !self.instance_signature.is_empty() {
            let _ = std::fs::remove_dir_all(self.runtime_dir.join("hypr").join(&self.instance_signature));
        }
    }
}

fn write_nested_config() -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("bread-capture-hypr-{}.lua", std::process::id()));
    // No exec-once/wallpaper daemon at all — that's the entire "no
    // background" mechanism (nothing ever claims the background layer).
    // misc.background_color matches bread-theme's own FIXED_BACKGROUND
    // (#0c0c0c, see bread-theme/src/palette.rs) so the empty canvas behind
    // a capture reads as "the app's own dark theme", not an arbitrary color.
    // disable_hyprland_logo/disable_splash_rendering turn off Hyprland's own
    // branded default background (drawn even with zero clients); the three
    // disable_* checks below turn off the on-screen red warning banner
    // Hyprland draws when started outside `start-hyprland`/without a
    // matching XDG_CURRENT_DESKTOP/without hyprland-dialog installed — all
    // expected and harmless here, but they'd otherwise show up in captures.
    let contents = r#"hl.monitor({
    output = "WAYLAND-1",
    scale = "1",
})
hl.config({
    misc = {
        disable_hyprland_logo = true,
        disable_splash_rendering = true,
        force_default_wallpaper = 0,
        background_color = "rgba(0c0c0cff)",
        disable_xdg_env_checks = true,
        disable_hyprland_guiutils_check = true,
        disable_watchdog_warning = true,
    },
})
"#;
    std::fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn dir_names(path: &Path) -> HashSet<String> {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

fn is_wayland_socket_name(name: &str) -> bool {
    name.strip_prefix("wayland-")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

fn poll_for_new(dir: &Path, before: &HashSet<String>, timeout: Duration, relevant: impl Fn(&str) -> bool) -> Result<String> {
    let start = Instant::now();
    loop {
        let after = dir_names(dir);
        if let Some(name) = after.iter().find(|n| relevant(n) && !before.contains(*n)) {
            return Ok(name.clone());
        }
        if start.elapsed() > timeout {
            bail!("timed out after {timeout:?} waiting for a new entry in {}", dir.display());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn poll_for_client_address(outer_signature: &str, pid: u32, timeout: Duration) -> Result<String> {
    let start = Instant::now();
    loop {
        if let Some(clients) = bread_utils::proc::run_json("hyprctl", &["--instance", outer_signature, "clients", "-j"], HYPRCTL_TIMEOUT) {
            if let Some(arr) = clients.as_array() {
                let found = arr
                    .iter()
                    .find(|c| c.get("pid").and_then(|v| v.as_u64()) == Some(pid as u64))
                    .and_then(|c| c.get("address"))
                    .and_then(|v| v.as_str());
                if let Some(addr) = found {
                    return Ok(addr.to_string());
                }
            }
        }
        if start.elapsed() > timeout {
            bail!("timed out after {timeout:?} waiting for pid {pid}'s window in the outer session's client list");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn outer_dispatch(outer_signature: &str, lua_expr: &str) -> Result<()> {
    let result = bread_utils::proc::run("hyprctl", &["--instance", outer_signature, "dispatch", lua_expr], HYPRCTL_TIMEOUT);
    if !result.success {
        bail!("hyprctl dispatch failed ({lua_expr}): {}", result.stderr.trim());
    }
    Ok(())
}
