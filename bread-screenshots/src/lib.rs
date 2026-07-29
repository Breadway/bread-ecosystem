//! Capture primitive for the bread ecosystem's UI screenshot tooling (see
//! `bread-capture`, the orchestrator that drives this crate's consumers).
//!
//! Deliberately compositor-agnostic: no Hyprland IPC, no layer/output
//! lookup. `bread-capture` runs every target app inside an isolated,
//! headless compositor instance of a known, fixed size (see its
//! `isolation` module), so the caller already knows exactly what region to
//! grab — there's nothing to query.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::time::Duration;

const GRIM_TIMEOUT: Duration = Duration::from_secs(5);

/// Capture a `w`x`h` region at `(x, y)` (compositor-global coordinates) to
/// `out` via `grim -g`.
pub fn capture_region(x: i32, y: i32, w: i32, h: i32, out: &Path) -> Result<()> {
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let out_str = out.to_str().context("output path is not valid UTF-8")?;
    let geometry = format!("{x},{y} {w}x{h}");
    let result = bread_utils::proc::run("grim", &["-g", &geometry, out_str], GRIM_TIMEOUT);
    if !result.success {
        bail!("grim failed for geometry {geometry}: {}", result.stderr.trim());
    }
    Ok(())
}
