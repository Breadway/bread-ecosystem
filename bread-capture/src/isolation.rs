//! Runs each capture target inside a headless Sway instance instead of the
//! operator's live desktop, so nothing on their screen (other windows, a
//! differently-themed real bar, whatever's behind a popover) can leak into a
//! capture, and the capture never flashes across their desktop either.
//!
//! This replaced an earlier nested-Hyprland approach (see git history for
//! `feature/capture-isolation` if you want the gory details). That worked,
//! but Hyprland's own backend library (Aquamarine) has no genuinely headless
//! mode when a live session already holds the seat — the only path was
//! nesting a full second Hyprland as an ordinary Wayland *client* of the
//! outer session, which meant: the outer compositor deciding the nested
//! window's pixel size (so every capture needed an outer-session
//! float+resize dispatch), the outer compositor throttling frame callbacks
//! for occluded surfaces (so the nested window also had to be *focused*, or
//! `grim` run inside it hung forever waiting on a frame that never came),
//! and — the thing that ultimately motivated dropping this approach — no way
//! to fully suppress the brief real, visible flash of that window on the
//! operator's actual screen (Lua-config Hyprland has no `keyword`-based
//! pre-emptive windowrule injection, and parking it on an untoggled special
//! workspace produced broken, half-rendered captures instead).
//!
//! wlroots (which Sway, not Hyprland, is built directly on) has a real
//! headless backend: `WLR_BACKENDS=headless` skips DRM and Wayland-client
//! backends entirely and synthesizes a virtual output with no seat/DRM-master
//! claim at all — no fight with logind over the live session's seat, and no
//! window anywhere, nested or otherwise, for the operator to ever see. Empirically
//! confirmed on this machine: zero visible footprint, `zwlr_layer_shell_v1`
//! and `zwlr_screencopy_manager_v1` both present (so a layer-shell bar and
//! `grim` both work), and a manual `grim` capture against it completes
//! instantly with no focus/occlusion dance required.
//!
//! One consequence of not nesting inside Hyprland at all: breadbar's
//! workspace list (`src/bar/workspaces.rs`, via the `hyprland` crate) talks
//! to whatever `HYPRLAND_INSTANCE_SIGNATURE` points at. Left alone, that
//! still points at the operator's real, live Hyprland instance — a data leak
//! into an otherwise-isolated capture (real workspace names/count showing up
//! in a bar screenshot that's supposed to be clean). Sway has no equivalent
//! IPC this needs to keep working, so [`Isolation::start`] unsets it;
//! breadbar already has to tolerate a missing/dead Hyprland connection
//! gracefully (it survives Hyprland restarting), so this just exercises that
//! same fallback path instead of a real error case.
//!
//! The background is a generated rainbow gradient, not a flat colour —
//! deliberately: a solid fill can't tell you whether a window that's
//! *supposed* to be translucent (breadbox/breadclip/breadsearch's
//! full-screen overlay windows, breadbar's notification/OSD surfaces) is
//! actually compositing as translucent, since a flat colour showing through
//! a flat colour still just looks flat. A continuously-varying gradient
//! makes any real transparency immediately obvious (multiple hues bleed
//! through) and any accidentally-opaque surface just as obvious (it blocks
//! the gradient out entirely, a flat rectangle where there should be colour
//! variation).

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Isolation {
    child: Child,
    pub wayland_display: String,
    config_path: PathBuf,
    background_path: PathBuf,
    runtime_dir: PathBuf,
}

impl Isolation {
    /// Spawn the headless instance sized to `width`x`height`, and set
    /// `WAYLAND_DISPLAY` on *this* process's own environment (and unset
    /// `HYPRLAND_INSTANCE_SIGNATURE`) so every subsequent
    /// `bread_utils::proc::run` spawn (the target app, and in turn its own
    /// `grim` calls) inherits them and lands inside the isolated instance.
    pub fn start(width: u32, height: u32) -> Result<Self> {
        let runtime_dir = PathBuf::from(
            std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string()),
        );
        let background_path = write_rainbow_background(width, height)?;
        let config_path = write_headless_config(width, height, &background_path)?;
        let before_sockets: HashSet<String> = dir_names(&runtime_dir)
            .into_iter()
            .filter(|n| is_wayland_socket_name(n))
            .collect();

        let child = Command::new("sway")
            .arg("-c")
            .arg(&config_path)
            .env("WLR_BACKENDS", "headless")
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawning headless sway")?;

        let mut isolation = Isolation {
            child,
            wayland_display: String::new(),
            config_path,
            background_path,
            runtime_dir: runtime_dir.clone(),
        };

        match poll_for_new(&runtime_dir, &before_sockets, DISCOVERY_TIMEOUT, is_wayland_socket_name)
            .context("waiting for headless sway's Wayland socket to appear")
        {
            Ok(name) => isolation.wayland_display = name,
            Err(e) => {
                // Best-effort teardown of the half-started instance before
                // propagating — the normal Drop impl still runs too, but
                // doing it here as well means a failure this early doesn't
                // depend on isolation ever being bound to a variable that
                // outlives this function.
                let _ = isolation.child.kill();
                let _ = isolation.child.wait();
                return Err(e);
            }
        }

        std::env::set_var("WAYLAND_DISPLAY", &isolation.wayland_display);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");

        Ok(isolation)
    }
}

impl Drop for Isolation {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.config_path);
        let _ = std::fs::remove_file(&self.background_path);
        // Killing sway doesn't unlink the socket it bound — confirmed
        // empirically, a killed instance leaves both files behind — so
        // without this, every capture run permanently orphans a
        // `wayland-N`/`wayland-N.lock` pair in the runtime dir.
        if !self.wayland_display.is_empty() {
            let _ = std::fs::remove_file(self.runtime_dir.join(&self.wayland_display));
            let _ = std::fs::remove_file(self.runtime_dir.join(format!("{}.lock", self.wayland_display)));
        }
    }
}

fn write_headless_config(width: u32, height: u32, background_path: &Path) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("bread-capture-sway-{}.conf", std::process::id()));
    let bg = background_path.display();
    let contents = format!(
        "output HEADLESS-1 resolution {width}x{height}\n\
         output HEADLESS-1 bg {bg} stretch\n"
    );
    std::fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Renders a diagonal rainbow (full hue sweep, both x and y contribute) to a
/// PNG at exactly `width`x`height`, for use as the isolated canvas's
/// background — see the module doc for why a gradient instead of a flat
/// colour. Diagonal rather than a simple left-to-right sweep so a capture
/// showing color variation isn't just luck-of-the-x-position: a purely
/// horizontal gradient would still make a tall, narrow surface look like a
/// near-flat single hue.
fn write_rainbow_background(width: u32, height: u32) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("bread-capture-bg-{}.png", std::process::id()));
    let mut img = image::RgbImage::new(width.max(1), height.max(1));
    let denom = (width + height).max(1) as f32;
    for y in 0..img.height() {
        for x in 0..img.width() {
            let hue = ((x + y) as f32 / denom) * 360.0;
            img.put_pixel(x, y, image::Rgb(hsv_to_rgb(hue, 0.85, 0.95)));
        }
    }
    img.save(&path).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Standard HSV -> RGB conversion. `h` in degrees [0, 360), `s`/`v` in [0, 1].
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let h_prime = (h / 60.0) % 6.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match h_prime as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r1 + m) * 255.0).round() as u8,
        ((g1 + m) * 255.0).round() as u8,
        ((b1 + m) * 255.0).round() as u8,
    ]
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

fn poll_for_new(
    dir: &Path,
    before: &HashSet<String>,
    timeout: Duration,
    relevant: impl Fn(&str) -> bool,
) -> Result<String> {
    let start = Instant::now();
    loop {
        let after = dir_names(dir);
        if let Some(name) = after.iter().find(|n| relevant(n) && !before.contains(*n)) {
            return Ok(name.clone());
        }
        if start.elapsed() > timeout {
            bail!(
                "timed out after {timeout:?} waiting for a new entry in {}",
                dir.display()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
