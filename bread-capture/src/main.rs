//! Orchestrator for the bread ecosystem's UI screenshot tooling.
//!
//! Drives each target app's `--screenshot <view> --output <path>` mode (see
//! `bread-screenshots` for what that mode does inside the app) and reports
//! pass/fail per view. One app per invocation, selected by `--app-name`
//! (defaults to `--app-path`'s file stem, so `--app-path
//! ./target/release/breadbox` needs no separate `--app-name`) — the view
//! list for each app is looked up from [`TARGETS`] below. Flat output
//! directory for now — no versioned `screenshots/vX.Y.Z/latest` structure
//! or manifest file yet, since that's still not earning its complexity over
//! a handful of apps.
//!
//! By default every capture runs inside a throwaway headless Sway instance
//! (see [`isolation`]) rather than the operator's live desktop, so another
//! window (or their own differently-themed real bar) can't leak into a
//! capture. `--no-isolate` skips that and captures directly against whatever
//! session bread-capture itself is running in — useful for debugging the
//! capture sequence itself, since you can then actually watch it happen.

mod isolation;

use anyhow::{bail, Result};
use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-app (view name, output filename) lists. Keyed by the app's binary
/// name — see `--app-name`.
const TARGETS: &[(&str, &[(&str, &str)])] = &[
    (
        "breadbar",
        &[
            ("bar", "breadbar-bar.png"),
            ("control-panel", "breadbar-control-panel.png"),
            ("connectivity-wifi", "breadbar-connectivity-wifi.png"),
            ("connectivity-bluetooth", "breadbar-connectivity-bluetooth.png"),
            ("media-popover", "breadbar-media-popover.png"),
            ("notification", "breadbar-notification.png"),
            ("notification-critical", "breadbar-notification-critical.png"),
            ("osd-volume", "breadbar-osd-volume.png"),
            ("osd-brightness", "breadbar-osd-brightness.png"),
            ("wifi-add-dialog", "breadbar-wifi-add-dialog.png"),
        ],
    ),
    ("breadbox", &[("launcher", "breadbox-launcher.png")]),
    ("breadclip", &[("history", "breadclip-history.png")]),
    ("breadsearch", &[("search", "breadsearch-search.png")]),
    (
        "breadpad",
        &[
            ("popup", "breadpad-popup.png"),
            ("reminder", "breadpad-reminder.png"),
            ("reminder-snooze", "breadpad-reminder-snooze.png"),
        ],
    ),
    (
        "breadhelp",
        &[
            ("home", "breadhelp-home.png"),
            ("learn", "breadhelp-learn.png"),
            ("ask", "breadhelp-ask.png"),
            ("troubleshoot-wizard", "breadhelp-troubleshoot-wizard.png"),
        ],
    ),
    (
        "breadman",
        &[
            ("all", "breadman-all.png"),
            ("upcoming", "breadman-upcoming.png"),
            ("todo", "breadman-todo.png"),
            ("reminder", "breadman-reminder.png"),
            ("idea", "breadman-idea.png"),
            ("note", "breadman-note.png"),
            ("question", "breadman-question.png"),
            ("archive", "breadman-archive.png"),
            ("settings", "breadman-settings.png"),
            ("errors", "breadman-errors.png"),
            ("editor", "breadman-editor.png"),
            ("new-note", "breadman-new-note.png"),
        ],
    ),
    (
        "bos-settings",
        &[
            ("network", "bos-settings-network.png"),
            ("breadcrumbs", "bos-settings-breadcrumbs.png"),
            ("bluetooth", "bos-settings-bluetooth.png"),
            ("firewall", "bos-settings-firewall.png"),
            ("sound", "bos-settings-sound.png"),
            ("power", "bos-settings-power.png"),
            ("datetime", "bos-settings-datetime.png"),
            ("hyprland", "bos-settings-hyprland.png"),
            ("keybinds", "bos-settings-keybinds.png"),
            ("autostart", "bos-settings-autostart.png"),
            ("users", "bos-settings-users.png"),
            ("appearance", "bos-settings-appearance.png"),
            ("breadpaper", "bos-settings-breadpaper.png"),
            ("breadbar", "bos-settings-breadbar.png"),
            ("breadbox", "bos-settings-breadbox.png"),
            ("breadclip", "bos-settings-breadclip.png"),
            ("breadpad", "bos-settings-breadpad.png"),
            ("breadsearch", "bos-settings-breadsearch.png"),
            ("bread", "bos-settings-bread.png"),
            ("packages", "bos-settings-packages.png"),
            ("aur", "bos-settings-aur.png"),
            ("firmware", "bos-settings-firmware.png"),
            ("snapshots", "bos-settings-snapshots.png"),
            ("about", "bos-settings-about.png"),
        ],
    ),
];

#[derive(Parser)]
struct Cli {
    /// Path to the target app's binary (resolved via $PATH if not a path).
    #[arg(long)]
    app_path: String,

    /// Which app's view list to use (see `TARGETS`). Defaults to
    /// `--app-path`'s file stem, e.g. `./target/release/breadbox` -> `breadbox`.
    #[arg(long)]
    app_name: Option<String>,

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

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();

    let app_name = cli.app_name.clone().unwrap_or_else(|| {
        PathBuf::from(&cli.app_path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| cli.app_path.clone())
    });
    let Some((_, views)) = TARGETS.iter().find(|(name, _)| *name == app_name) else {
        bail!(
            "no known view list for app '{app_name}' (known: {})",
            TARGETS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
        );
    };

    // Bound, not dropped-and-discarded: `_isolation`'s teardown (kill the
    // compositor, remove its socket/config) must run via Drop regardless of
    // how this function returns below — returning an ExitCode rather than
    // calling `std::process::exit` (which skips destructors entirely) is
    // what makes that true on the failure path too.
    let _isolation = if cli.no_isolate {
        None
    } else {
        Some(isolation::Isolation::start(cli.isolate_width, cli.isolate_height)?)
    };

    let width_str = cli.isolate_width.to_string();
    let height_str = cli.isolate_height.to_string();

    let mut failed = false;
    for (view, filename) in *views {
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
            println!("ok    {app_name}/{view} -> {}", out_path.display());
        } else {
            failed = true;
            println!("FAIL  {app_name}/{view}: {}", result.stderr.trim());
        }
    }

    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}
