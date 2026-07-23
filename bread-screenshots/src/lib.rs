//! Capture primitives for the bread ecosystem's UI screenshot tooling (see
//! `bread-capture`, the orchestrator that drives this crate's consumers).
//!
//! A "view" being screenshotted is either:
//! - its own layer-shell surface (a bar, launcher, ...) — captured tightly via
//!   [`capture_layer`], geometry from `hyprctl layers`.
//! - a transient popover/popup — not a separate layer surface under
//!   gtk4-layer-shell (it's an xdg_popup Hyprland doesn't list individually),
//!   so the only reliable capture is [`capture_output`]: whatever's currently
//!   on the focused monitor.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::time::Duration;

const GRIM_TIMEOUT: Duration = Duration::from_secs(5);

/// Capture a named layer-shell surface belonging to *this* process
/// (`std::process::id()`), identified by its `namespace` (e.g. `"breadbar"`).
/// Namespace alone can't identify "our own" surface when another instance
/// under the same namespace is already running (breadbar commonly is), so
/// this matches by pid too — see `bread_utils::hypr::find_layer`.
pub fn capture_layer(namespace: &str, out: &Path) -> Result<()> {
    let layer = bread_utils::hypr::find_layer(namespace, std::process::id())
        .with_context(|| format!("no layer surface found for namespace={namespace}"))?;
    let geometry = format!("{},{} {}x{}", layer.x, layer.y, layer.w, layer.h);
    run_grim(&geometry, out)
}

/// Capture the entire focused output (monitor). Used for views whose
/// interesting content isn't its own layer surface — see the module doc.
pub fn capture_output(out: &Path) -> Result<()> {
    let monitor = bread_utils::hypr::focused_monitor().context("no focused monitor found")?;
    let (w, h) = monitor.logical_size();
    let geometry = format!("{},{} {}x{}", monitor.x, monitor.y, w, h);
    run_grim(&geometry, out)
}

fn run_grim(geometry: &str, out: &Path) -> Result<()> {
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let out_str = out.to_str().context("output path is not valid UTF-8")?;
    let result = bread_utils::proc::run("grim", &["-g", geometry, out_str], GRIM_TIMEOUT);
    if !result.success {
        bail!("grim failed for geometry {geometry}: {}", result.stderr);
    }
    Ok(())
}
