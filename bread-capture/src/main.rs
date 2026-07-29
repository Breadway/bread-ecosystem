//! Orchestrator for the bread ecosystem's UI screenshot tooling.
//!
//! Drives each target app's `--screenshot <view> --output <path>` mode (see
//! `bread-screenshots` for what that mode does inside the app) and reports
//! pass/fail per view/app. Plain `bread-capture` with no flags captures
//! every known app's every view in one run — each app's binary is resolved
//! by its own bare name via `$PATH`, same as running it directly by name
//! would. `--app <name>` restricts to one app; `--app-path <path>`
//! overrides where its binary is found (and, without `--app`, also selects
//! which app by its file stem — so `--app-path ./target/release/breadbox`
//! alone still works); `--view <name>` further restricts to one view. The
//! view list for each app is looked up from [`TARGETS`] below. Flat output
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
    /// Restrict to one app (see `TARGETS` for known names). Omit to capture
    /// every known app's every view in one run.
    #[arg(long)]
    app: Option<String>,

    /// Path to that app's binary (resolved via $PATH if not a path).
    /// Without `--app`, this also selects *which* app by its file stem
    /// (e.g. `./target/release/breadbox` -> `breadbox`) — so a single-app
    /// run never needs both flags. Ignored (with a warning) if given
    /// together with a multi-app run (no `--app`, and the path isn't
    /// resolvable to exactly one app).
    #[arg(long)]
    app_path: Option<String>,

    /// Restrict to one view within the selected app(s) (see each app's
    /// entry in `TARGETS` for known view names). Apps that don't have a
    /// view by this name are skipped, not treated as an error, since a
    /// multi-app run's view names naturally don't all overlap.
    #[arg(long)]
    view: Option<String>,

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

fn known_app_names() -> String {
    TARGETS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
}

/// (app_name, binary_path, views) per selected app.
type SelectedTarget = (&'static str, String, &'static [(&'static str, &'static str)]);

/// Resolves which `TARGETS` entries this run covers, and the binary path
/// to use for each.
fn selected_targets(cli: &Cli) -> Result<Vec<SelectedTarget>> {
    if let Some(app) = &cli.app {
        let Some((name, views)) = TARGETS.iter().find(|(n, _)| n == app) else {
            bail!("no known view list for app '{app}' (known: {})", known_app_names());
        };
        let path = cli.app_path.clone().unwrap_or_else(|| name.to_string());
        return Ok(vec![(name, path, views)]);
    }

    if let Some(path) = &cli.app_path {
        let stem = PathBuf::from(path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let Some((name, views)) = TARGETS.iter().find(|(n, _)| *n == stem) else {
            bail!("no known view list for app '{stem}' (known: {})", known_app_names());
        };
        return Ok(vec![(name, path.clone(), views)]);
    }

    // No --app / --app-path at all: every known app, resolved by its own
    // bare name via $PATH.
    Ok(TARGETS.iter().map(|(name, views)| (*name, name.to_string(), *views)).collect())
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    let targets = selected_targets(&cli)?;

    if let Some(view) = &cli.view {
        if !targets.iter().any(|(_, _, views)| views.iter().any(|(v, _)| v == view)) {
            bail!("view '{view}' doesn't match any selected app's views");
        }
    }

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
    for (app_name, app_path, views) in &targets {
        for (view, filename) in *views {
            if cli.view.as_deref().is_some_and(|v| v != *view) {
                continue;
            }
            let out_path = cli.out_dir.join(filename);
            let out_str = out_path.to_string_lossy();
            let result = bread_utils::proc::run(
                app_path,
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
    }

    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}
