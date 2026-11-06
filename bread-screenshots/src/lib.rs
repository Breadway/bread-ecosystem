//! Capture primitive for the bread ecosystem's UI screenshot tooling (see
//! `bread-capture`, the orchestrator that drives this crate's consumers).
//!
//! Deliberately compositor-agnostic: no Hyprland IPC, no layer/output
//! lookup. `bread-capture` runs every target app inside an isolated,
//! headless compositor instance of a known, fixed size (see its
//! `isolation` module), so the caller already knows exactly what region to
//! grab; there's nothing to query.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::time::Duration;

const GRIM_TIMEOUT: Duration = Duration::from_secs(30);

/// Capture a `w`x`h` region at `(x, y)` (compositor-global coordinates) to
/// `out` via `grim -g`.
pub fn capture_region(x: i32, y: i32, w: i32, h: i32, out: &Path) -> Result<()> {
    capture_region_with("grim", x, y, w, h, out, GRIM_TIMEOUT)
}

fn capture_region_with(
    executable: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    out: &Path,
    timeout: Duration,
) -> Result<()> {
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let out_str = out.to_str().context("output path is not valid UTF-8")?;
    let geometry = format!("{x},{y} {w}x{h}");
    let result = bread_utils::proc::run_detailed(executable, &["-g", &geometry, out_str], timeout);
    if !result.output.success {
        let _ = std::fs::remove_file(out);
        if result.timed_out {
            bail!(
                "grim timed out after {:.1}s for geometry {geometry}",
                timeout.as_secs_f64()
            );
        }
        let stderr = result.output.stderr.trim();
        bail!(
            "grim failed for geometry {geometry}: {}",
            if stderr.is_empty() {
                "no error output"
            } else {
                stderr
            }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn timed_out_capture_removes_partial_output_and_reports_timeout() {
        let temp = tempfile::tempdir().unwrap();
        let fake_grim = temp.path().join("grim");
        std::fs::write(
            &fake_grim,
            "#!/bin/sh\nprintf partial > \"$3\"\nexec sleep 10\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_grim).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_grim, permissions).unwrap();

        let output = temp.path().join("capture.png");
        let error = capture_region_with(
            fake_grim.to_str().unwrap(),
            0,
            0,
            1920,
            1080,
            &output,
            Duration::from_millis(100),
        )
        .unwrap_err();

        assert!(error.to_string().contains("timed out after 0.1s"));
        assert!(!output.exists(), "partial capture was left behind");
    }

    #[test]
    fn failed_capture_removes_partial_output_and_reports_stderr() {
        let temp = tempfile::tempdir().unwrap();
        let fake_grim = temp.path().join("grim");
        std::fs::write(
            &fake_grim,
            "#!/bin/sh\nprintf partial > \"$3\"\necho capture-error >&2\nexit 1\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_grim).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_grim, permissions).unwrap();

        let output = temp.path().join("capture.png");
        let error = capture_region_with(
            fake_grim.to_str().unwrap(),
            0,
            0,
            1920,
            1080,
            &output,
            Duration::from_secs(2),
        )
        .unwrap_err();

        assert!(error.to_string().contains("capture-error"));
        assert!(!output.exists(), "partial capture was left behind");
    }
}
