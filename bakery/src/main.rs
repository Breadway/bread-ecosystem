mod doctor;
mod download;
mod install;
mod manifest;
mod state;
mod track;
mod ui;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::HashSet;
use std::path::PathBuf;
use track::Track;

#[derive(Parser)]
#[command(name = "bakery", about = "Package manager for the bread ecosystem", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
    /// Override the directory where binaries are installed
    #[arg(long, env = "BAKERY_BIN_DIR", global = true)]
    bin_dir: Option<PathBuf>,
    /// Skip post_install hooks entirely
    #[arg(long, global = true)]
    no_hooks: bool,
    /// Assume yes to interactive prompts
    #[arg(short = 'y', long = "yes", global = true)]
    yes: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install one or more packages
    Install {
        #[arg(required = true, num_args = 1..)]
        packages: Vec<String>,
    },
    /// Remove an installed package (data files are never deleted)
    Remove {
        package: String,
    },
    /// Update one or all installed packages
    Update {
        /// Package to update (omit or use --all to update everything installed)
        #[arg(conflicts_with = "all")]
        package: Option<String>,
        /// Update all installed packages
        #[arg(long, conflicts_with = "package")]
        all: bool,
    },
    /// List packages
    List {
        /// Show only installed packages
        #[arg(long)]
        installed: bool,
    },
    /// Show details for a package
    Info {
        package: String,
    },
    /// Check system dependencies for installed or requested packages
    Doctor {
        /// Package to check; omit to check all installed packages
        package: Option<String>,
    },
    /// View or switch which build track bakery follows (stable/beta/dev)
    Track {
        #[command(subcommand)]
        action: TrackCmd,
    },
}

#[derive(Subcommand)]
enum TrackCmd {
    /// Show the currently selected track
    Show,
    /// Switch tracks. Only changes the preference — run `bakery update --all`
    /// afterwards to actually install builds from the new track.
    Set { track: Track },
}

fn default_bin_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".local/bin")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let bin_dir = cli.bin_dir.unwrap_or_else(default_bin_dir);
    let no_hooks = cli.no_hooks;
    let assume_yes = cli.yes;
    let track = state::State::load()?.track;

    match cli.command {
        Cmd::Install { packages } => {
            let index = manifest::load(true, track)?;
            for pkg in &packages {
                cmd_install(&index, pkg, &bin_dir, track, no_hooks, assume_yes)?;
            }
            Ok(())
        }
        Cmd::Remove { package } => cmd_remove(&package, &bin_dir, assume_yes),
        Cmd::Update { package, all } => {
            cmd_update(package.as_deref(), all, &bin_dir, track, no_hooks, assume_yes)
        }
        Cmd::List { installed } => cmd_list(installed, track),
        Cmd::Info { package } => cmd_info(&package, track),
        Cmd::Doctor { package } => cmd_doctor(package.as_deref(), track, &bin_dir),
        Cmd::Track { action } => cmd_track(action),
    }
}

fn cmd_track(action: TrackCmd) -> Result<()> {
    let state = state::State::load()?;
    match action {
        TrackCmd::Show => {
            println!("current track: {}", ui::style(state.track.as_str(), ui::CYAN));
        }
        TrackCmd::Set { track } => {
            if state.track == track {
                println!("already on track {track}");
                return Ok(());
            }
            // Fail fast on a bad/unreachable track rather than silently
            // recording a preference bakery can't actually serve.
            manifest::load(true, track)
                .with_context(|| format!("could not validate {track} track, not switching"))?;
            state::State::with_lock(|state| {
                state.set_track(track);
                Ok(())
            })?;
            println!(
                "switched to {} — run 'bakery update --all' to install {} builds",
                ui::style(track.as_str(), ui::CYAN),
                track
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_install(
    index: &manifest::Index,
    name: &str,
    bin_dir: &std::path::Path,
    track: Track,
    no_hooks: bool,
    assume_yes: bool,
) -> Result<()> {
    let mut visited = HashSet::new();
    install_with_deps(index, name, bin_dir, track, no_hooks, assume_yes, &mut visited)
}

/// Recursively installs `name` and any bread_deps, skipping already-installed
/// packages. The `visited` set prevents cycles.
#[allow(clippy::too_many_arguments)]
fn install_with_deps(
    index: &manifest::Index,
    name: &str,
    bin_dir: &std::path::Path,
    track: Track,
    no_hooks: bool,
    assume_yes: bool,
    visited: &mut HashSet<String>,
) -> Result<()> {
    if !visited.insert(name.to_string()) {
        return Ok(());
    }

    let pkg = index
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("unknown package: {name}"))?;

    // Install bread_deps first (skip those already recorded in state).
    let state = state::State::load()?;
    for dep in pkg.bread_deps.clone() {
        if !state.is_installed(&dep) {
            println!("installing bread dependency: {dep}");
            install_with_deps(index, &dep, bin_dir, track, no_hooks, assume_yes, visited)?;
        }
    }

    // Already installed and not older than the index — nothing to do. This
    // doubles as the implicit upgrade path when the index has something
    // newer, so `bakery install <pkg>` is safe to run repeatedly instead of
    // silently reinstalling (and potentially downgrading) every time.
    if let Some(installed) = state.packages.get(name) {
        if !is_newer(&installed.version, &pkg.version) {
            println!(
                "{name} already installed at {} (index has {})",
                installed.version, pkg.version
            );
            return Ok(());
        }
    }

    println!("checking system dependencies for {name}…");
    let rep = doctor::check_deps(&pkg.system_deps, &pkg.optional_system_deps)?;
    for warn in &rep.warnings {
        eprintln!("  note: optional dep not installed: {warn}");
    }
    if !rep.missing.is_empty() {
        eprintln!("missing system deps for {name}: {}", rep.missing.join(", "));
        eprintln!("install with: sudo pacman -S {}", rep.missing.join(" "));
        bail!("system deps not satisfied");
    }

    install::install_package(pkg, bin_dir, track, no_hooks, assume_yes)
}

fn cmd_remove(name: &str, bin_dir: &std::path::Path, assume_yes: bool) -> Result<()> {
    install::remove_package(name, bin_dir, assume_yes)
}

#[allow(clippy::too_many_arguments)]
fn cmd_update(
    name: Option<&str>,
    all: bool,
    bin_dir: &std::path::Path,
    track: Track,
    no_hooks: bool,
    assume_yes: bool,
) -> Result<()> {
    let index = manifest::load(true, track)?;
    let state = state::State::load()?;

    let targets: Vec<String> = if all || name.is_none() {
        state.packages.keys().cloned().collect()
    } else {
        vec![name.unwrap().to_string()]
    };

    if targets.is_empty() {
        println!("no packages installed");
        return Ok(());
    }

    let mut any_failed = false;
    for pkg_name in &targets {
        let installed = match state.packages.get(pkg_name.as_str()) {
            Some(p) => p,
            None => {
                eprintln!("{pkg_name} is not installed, skipping");
                any_failed = true;
                continue;
            }
        };
        let latest = match index.get(pkg_name) {
            Some(p) => p,
            None => {
                eprintln!("{pkg_name} not found in index, skipping");
                any_failed = true;
                continue;
            }
        };

        // A track switch is an explicit user action ("bakery track set beta
        // && bakery update --all") and must always take effect, even if the
        // new track's current build happens to be same-or-lower by strict
        // semver than what's installed (e.g. switching stable -> dev, or a
        // beta RC that shares a base version with the installed stable).
        let track_switch = installed.track != track;
        if !should_update(&installed.version, installed.track, track, &latest.version) {
            println!("{}", ui::style(&format!("{pkg_name} is already at {}", installed.version), ui::GREEN));
            continue;
        }

        if track_switch {
            println!(
                "{pkg_name} switching track {} {} {}, installing {}",
                ui::style(installed.track.as_str(), ui::DIM),
                ui::style("→", ui::CYAN),
                ui::style(track.as_str(), ui::BOLD),
                ui::style(&latest.version, ui::BOLD)
            );
        } else {
            println!(
                "updating {pkg_name} {} {} {}",
                ui::style(&installed.version, ui::DIM),
                ui::style("→", ui::CYAN),
                ui::style(&latest.version, ui::BOLD)
            );
        }

        let rep = match doctor::check_deps(&latest.system_deps, &latest.optional_system_deps) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("  doctor check failed for {pkg_name}: {e}");
                any_failed = true;
                continue;
            }
        };
        for warn in &rep.warnings {
            eprintln!("  note: optional dep not installed: {warn}");
        }
        if !rep.missing.is_empty() {
            eprintln!(
                "  missing deps for {pkg_name}: {} — skipping update",
                rep.missing.join(", ")
            );
            any_failed = true;
            continue;
        }

        if let Err(e) = install::install_package(latest, bin_dir, track, no_hooks, assume_yes) {
            eprintln!("  failed to update {pkg_name}: {e}");
            any_failed = true;
        }
    }

    if any_failed {
        bail!("one or more packages could not be updated");
    }
    Ok(())
}

/// Whether `pkg_name` should be updated: always true on a track switch
/// (an explicit user action that must take effect regardless of version
/// ordering), otherwise a real semver comparison via [`is_newer`].
fn should_update(installed_version: &str, installed_track: Track, active_track: Track, latest_version: &str) -> bool {
    if installed_track != active_track {
        return true;
    }
    is_newer(installed_version, latest_version)
}

/// Is `latest` newer than `installed`? Real semver comparison — the
/// previous plain string-equality check couldn't tell "different" from
/// "actually newer", so it would happily "update" a package to a lexically
/// different but not-newer version. Falls back to a simple inequality check
/// (with a warning) for any version string that isn't valid semver, rather
/// than hard-erroring on packages built before this convention existed.
fn is_newer(installed: &str, latest: &str) -> bool {
    match (semver::Version::parse(installed), semver::Version::parse(latest)) {
        (Ok(i), Ok(l)) => l > i,
        _ => {
            if installed != latest {
                eprintln!(
                    "  warning: cannot determine if '{latest}' is newer than '{installed}' \
                     (not valid semver) — proceeding on inequality alone, this could be a downgrade"
                );
            }
            installed != latest
        }
    }
}

fn cmd_list(installed_only: bool, track: Track) -> Result<()> {
    let state = state::State::load()?;

    if installed_only {
        if state.packages.is_empty() {
            println!("no packages installed");
        }
        for pkg in state.packages.values() {
            println!("  {} {} (installed {})", pkg.name, pkg.version, pkg.installed_at);
        }
        return Ok(());
    }

    if !matches!(track, Track::Stable) {
        println!("tracking:{}\n", ui::track_badge(track));
    }

    let index = manifest::load(false, track)?;
    let mut names: Vec<&str> = index.packages.keys().map(|s| s.as_str()).collect();
    names.sort();
    for name in names {
        let pkg = &index.packages[name];
        let tag = if state.is_installed(name) {
            ui::style(&format!(" [installed {}]", state.packages[name].version), ui::GREEN)
        } else {
            String::new()
        };
        println!("  {:<14} {:<10} — {}{}", pkg.name, pkg.version, pkg.description, tag);
    }
    Ok(())
}

fn cmd_info(name: &str, track: Track) -> Result<()> {
    let index = manifest::load(false, track)?;
    let pkg = index
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("unknown package: {name}"))?;

    let state = state::State::load()?;
    let status = if let Some(inst) = state.packages.get(name) {
        ui::style(&format!("installed ({})", inst.version), ui::GREEN)
    } else {
        ui::style("not installed", ui::DIM)
    };

    println!("{}{} {}", ui::style(&pkg.name, ui::BOLD), ui::track_badge(track), pkg.version);
    println!("  {}", pkg.description);
    println!("  status:      {status}");
    println!(
        "  binaries:    {}",
        pkg.binaries
            .iter()
            .map(|b| b.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !pkg.system_deps.is_empty() {
        println!("  system deps: {}", pkg.system_deps.join(", "));
    }
    if !pkg.optional_system_deps.is_empty() {
        println!("  optional deps: {}", pkg.optional_system_deps.join(", "));
    }
    if !pkg.bread_deps.is_empty() {
        println!("  bread deps:  {}", pkg.bread_deps.join(", "));
    }
    if !pkg.services.is_empty() {
        println!(
            "  services:    {}",
            pkg.services
                .iter()
                .map(|s| s.unit.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

fn cmd_doctor(name: Option<&str>, track: Track, bin_dir: &std::path::Path) -> Result<()> {
    let index = manifest::load(false, track)?;
    let state = state::State::load()?;

    let targets: Vec<String> = match name {
        Some(n) => {
            if index.get(n).is_none() {
                bail!("unknown package: {n}");
            }
            vec![n.to_string()]
        }
        None => state.packages.keys().cloned().collect(),
    };

    if targets.is_empty() {
        println!("no packages installed — nothing to check");
        return Ok(());
    }

    let mut all_ok = true;
    for pkg_name in &targets {
        if let Some(pkg) = index.get(pkg_name) {
            if !doctor::report(pkg_name, &pkg.system_deps, &pkg.optional_system_deps) {
                all_ok = false;
            }
        } else {
            eprintln!("  {pkg_name}: not found in index (removed from registry?)");
            all_ok = false;
        }

        // System-deps checks alone can't catch a partially-broken install
        // (e.g. a binary manually deleted after install) — also confirm
        // every binary this package recorded is still on disk. Existence
        // only, not a checksum re-verification — that's the scope of a
        // future `bakery verify` command.
        if let Some(installed) = state.packages.get(pkg_name) {
            for bin in &installed.binaries {
                let path = bin_dir.join(bin);
                if !path.exists() {
                    eprintln!(
                        "  {}",
                        ui::fail(&format!(
                            "{pkg_name}: recorded binary '{bin}' is missing at {}",
                            path.display()
                        ))
                    );
                    all_ok = false;
                }
            }
        }
    }

    if all_ok {
        println!("{}", ui::ok("all checks passed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_detects_real_semver_increase() {
        assert!(is_newer("0.3.1", "0.3.2"));
        assert!(is_newer("0.3.1", "0.4.0"));
        assert!(!is_newer("0.3.2", "0.3.1"));
    }

    #[test]
    fn is_newer_false_when_equal() {
        assert!(!is_newer("0.3.1", "0.3.1"));
    }

    #[test]
    fn is_newer_orders_dev_prereleases_within_a_track() {
        // Two dev builds of the same upcoming patch, ordered by their
        // timestamp+sha build suffix.
        assert!(is_newer(
            "0.3.2-dev.20260722120000+aaa1111",
            "0.3.2-dev.20260722130000+bbb2222"
        ));
    }

    #[test]
    fn is_newer_falls_back_to_inequality_on_unparseable_versions() {
        // Pre-semver version strings should never hard-fail an update check.
        assert!(is_newer("weird-version-1", "weird-version-2"));
        assert!(!is_newer("weird-version-1", "weird-version-1"));
    }

    #[test]
    fn should_update_true_on_track_switch_even_if_not_newer_by_semver() {
        // "bakery track set stable && bakery update --all" from beta must
        // always take effect, even though 0.3.0 < 0.4.0-beta by strict semver.
        assert!(should_update("0.4.0-beta", Track::Beta, Track::Stable, "0.3.0"));
    }

    #[test]
    fn should_update_false_when_same_track_and_not_newer() {
        assert!(!should_update("0.3.1", Track::Stable, Track::Stable, "0.3.1"));
        assert!(!should_update("0.3.2", Track::Dev, Track::Dev, "0.3.1"));
    }

    #[test]
    fn should_update_true_when_same_track_and_newer() {
        assert!(should_update("0.3.1", Track::Stable, Track::Stable, "0.3.2"));
    }
}
