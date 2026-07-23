//! Orchestrator for the bread ecosystem's UI screenshot tooling.
//!
//! Drives each target app's `--screenshot <view> --output <path>` mode (see
//! `bread-screenshots` for what that mode does inside the app) and reports
//! pass/fail per view. Foundation-phase scope: one target (breadbar), a
//! hardcoded view list, and a flat output directory — no versioned
//! `screenshots/vX.Y.Z/latest` structure or manifest file yet, since those
//! only earn their complexity once more apps are wired up.

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut failed = false;
    for (view, filename) in BREADBAR_TARGETS {
        let out_path = cli.out_dir.join(filename);
        let out_str = out_path.to_string_lossy();
        let result = bread_utils::proc::run(
            &cli.app_path,
            &["--screenshot", view, "--output", &out_str],
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
