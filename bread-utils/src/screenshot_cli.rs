//! Shared `--screenshot` CLI flags and the post-`map` settle delay used by
//! bread-capture-driven GTK apps.
//!
//! The same clap block (`--screenshot`, `--output`, `--width`, `--height`)
//! plus a 300ms settle after GTK `map` is cloned across breadbar, breadbox,
//! breadclip, breadpad, breadman, breadsearch, and breadhelp. This module
//! is the next-pin target for that duplication — consumers this cycle still
//! pin an older bread-utils tag and will not see it until a future release.
//!
//! Zero extra deps: no clap, no GTK. Apps keep (or flatten) the four `#[arg]`
//! fields themselves and call [`validate_pair`] / [`SETTLE_DELAY`].
//!
//! Confirmed present in:
//! - `breadbar/src/screenshot.rs`
//! - `breadbox/breadbox/src/screenshot.rs`
//! - `breadclip/breadclip/src/screenshot.rs`
//! - `breadsearch/breadsearch/src/screenshot.rs`
//! - `breadpad/breadpad/src/screenshot.rs`
//! - `breadpad/breadman/src/screenshot.rs`
//! - `breadhelp/src/screenshot.rs`
//!
//! Do not fold `bread-screenshots` or `bread-capture`'s `TARGETS` table into
//! this module — those are the capture primitive and the orchestrator, not
//! the per-app CLI flags.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Extra settle time after GTK `map` for the first frame to actually paint
/// before grim runs. `map` fires once the surface exists, not once anything
/// has been drawn into it.
pub const SETTLE_DELAY: Duration = Duration::from_millis(300);

/// Default `--width`, matching `bread-capture --isolate-width`.
pub const DEFAULT_WIDTH: u32 = 1920;

/// Default `--height`, matching `bread-capture --isolate-height`.
pub const DEFAULT_HEIGHT: u32 = 1080;

/// The four `--screenshot` / `--output` / `--width` / `--height` values
/// parsed from an app's CLI.
///
/// `screenshot` and `output` must both be present (a capture run) or both
/// absent (a normal run) — see [`ScreenshotCli::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenshotCli {
    /// Named view to capture (`--screenshot`). `None` for a normal run.
    pub screenshot: Option<String>,
    /// PNG path to write (`--output`). Required together with `screenshot`.
    pub output: Option<PathBuf>,
    /// Capture canvas width (`--width`).
    pub width: u32,
    /// Capture canvas height (`--height`).
    pub height: u32,
}

impl Default for ScreenshotCli {
    fn default() -> Self {
        Self {
            screenshot: None,
            output: None,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
        }
    }
}

/// Why a screenshot-flag pair is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenshotCliError {
    /// `--screenshot` was given without `--output`.
    ScreenshotWithoutOutput,
    /// `--output` was given without `--screenshot`.
    OutputWithoutScreenshot,
}

impl std::fmt::Display for ScreenshotCliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ScreenshotWithoutOutput => write!(f, "--screenshot requires --output"),
            Self::OutputWithoutScreenshot => write!(f, "--output requires --screenshot"),
        }
    }
}

impl std::error::Error for ScreenshotCliError {}

/// Both `--screenshot` and `--output` must be present, or neither.
pub fn validate_pair(
    screenshot: Option<&str>,
    output: Option<&Path>,
) -> Result<(), ScreenshotCliError> {
    match (screenshot, output) {
        (Some(_), Some(_)) | (None, None) => Ok(()),
        (Some(_), None) => Err(ScreenshotCliError::ScreenshotWithoutOutput),
        (None, Some(_)) => Err(ScreenshotCliError::OutputWithoutScreenshot),
    }
}

impl ScreenshotCli {
    /// Both `screenshot` and `output` present, or neither.
    pub fn validate(&self) -> Result<(), ScreenshotCliError> {
        validate_pair(self.screenshot.as_deref(), self.output.as_deref())
    }

    /// `true` when this is a capture run (both flags present).
    pub fn is_screenshot_run(&self) -> bool {
        self.screenshot.is_some() && self.output.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settle_delay_is_300ms() {
        assert_eq!(SETTLE_DELAY, Duration::from_millis(300));
    }

    #[test]
    fn neither_flag_is_ok() {
        assert!(validate_pair(None, None).is_ok());
        assert!(ScreenshotCli::default().validate().is_ok());
        assert!(!ScreenshotCli::default().is_screenshot_run());
    }

    #[test]
    fn both_flags_are_ok() {
        let cli = ScreenshotCli {
            screenshot: Some("search".into()),
            output: Some(PathBuf::from("/tmp/out.png")),
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
        };
        assert!(cli.validate().is_ok());
        assert!(cli.is_screenshot_run());
    }

    #[test]
    fn screenshot_without_output_is_an_error() {
        assert_eq!(
            validate_pair(Some("search"), None),
            Err(ScreenshotCliError::ScreenshotWithoutOutput)
        );
    }

    #[test]
    fn output_without_screenshot_is_an_error() {
        let path = PathBuf::from("/tmp/out.png");
        assert_eq!(
            validate_pair(None, Some(path.as_path())),
            Err(ScreenshotCliError::OutputWithoutScreenshot)
        );
    }
}
