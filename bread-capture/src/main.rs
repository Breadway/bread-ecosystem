//! Orchestrator for the bread ecosystem's UI screenshot tooling.
//!
//! Drives each target app's `--screenshot <view> --output <path>` mode (see
//! `bread-screenshots` for what that mode does inside the app) and reports
//! pass/fail per view. Foundation-phase scope: one target (breadbar), a
//! hardcoded view list, and a flat output directory — no versioned
//! `screenshots/vX.Y.Z/latest` structure or manifest file yet, since those
//! only earn their complexity once more apps are wired up.
//!
//! By default every capture runs inside a throwaway headless Sway instance
//! (see [`isolation`]) rather than the operator's live desktop, so another
//! window (or their own differently-themed real bar) can't leak into a
//! capture. `--no-isolate` skips that and captures directly against whatever
//! session bread-capture itself is running in — useful for debugging the
//! capture sequence itself, since you can then actually watch it happen.

mod isolation;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::time::Duration;

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// (view name, output filename)
const BREADBAR_TARGETS: &[(&str, &str)] = &[
    ("bar", "breadbar-bar.png"),
    ("control-panel", "breadbar-control-panel.png"),
];

#[derive(Parser)]
struct Cli {
    /// Path to the breadbar binary (resolved via $PATH if not a path).
    #[arg(long, default_value = "breadbar")]
    app_path: String,

    /// Directory to write captured PNGs into.
    #[arg(long, default_value = "./screenshots")]
    out_dir: PathBuf,

    /// Capture directly against the current session instead of a headless,
    /// throwaway Sway instance. Off by default so captures can't pick up
    /// whatever else is on the operator's desktop.
    #[arg(long)]
    no_isolate: bool,

    /// Width of the isolated session's capture canvas.
    #[arg(long, default_value_t = 1920)]
    isolate_width: u32,

    /// Height of the isolated session's capture canvas.
    #[arg(long, default_value_t = 1080)]
    isolate_height: u32,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let _isolation = if cli.no_isolate {
        None
    } else {
        Some(isolation::Isolation::start(cli.isolate_width, cli.isolate_height)?)
    };

    let width_str = cli.isolate_width.to_string();
    let height_str = cli.isolate_height.to_string();

    let mut failed = false;
    for (view, filename) in BREADBAR_TARGETS {
        let out_path = cli.out_dir.join(filename);
        let out_str = out_path.to_string_lossy();
        let result = bread_utils::proc::run(
            &cli.app_path,
            &[
                "--screenshot", view,
                "--output", &out_str,
                "--width", &width_str,
                "--height", &height_str,
            ],
            CAPTURE_TIMEOUT,
        );
        if result.success {
            println!("ok    breadbar/{view} -> {}", out_path.display());
        } else {
            failed = true;
            println!("FAIL  breadbar/{view}: {}", result.stderr.trim());
        }
    }

    if failed {
        std::process::exit(1);
    }
    Ok(())
}
