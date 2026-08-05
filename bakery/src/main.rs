mod doctor;
mod download;
mod install;
mod manifest;
mod state;
mod track;
mod ui;

use anyhow::{bail, Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
    /// Show what would happen without downloading, writing, or touching state
    #[arg(long, global = true)]
    dry_run: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Install one or more packages
    Install {
        #[arg(required = true, num_args = 1..)]
        packages: Vec<String>,
    },
    /// Remove an installed package (config is never deleted)
    Remove {
        package: String,
        /// Also remove the license file, desktop entry, and data dir
        /// (~/.local/share/<pkg>/) — config is still preserved
        #[arg(long)]
        purge: bool,
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
    /// Search package names and descriptions
    Search {
        query: String,
    },
    /// Check system dependencies for installed or requested packages
    Doctor {
        /// Package to check; omit to check all installed packages
        package: Option<String>,
    },
    /// Verify installed binaries against the checksum recorded at install time
    Verify {
        /// Package to verify; omit to verify all installed packages
        package: Option<String>,
    },
    /// Roll back a package to its previously installed version, from a
    /// local pre-update backup (not a re-download)
    Rollback {
        package: String,
    },
    /// Update bakery itself
    SelfUpdate,
    /// Generate a shell completion script
    Completions {
        shell: clap_complete::Shell,
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
    let dry_run = cli.dry_run;
    let track = state::State::load()?.track;

    match cli.command {
        Cmd::Install { packages } => {
            let index = manifest::load(true, track)?;
            for pkg in &packages {
                cmd_install(&index, pkg, &bin_dir, track, no_hooks, assume_yes, dry_run)?;
            }
            Ok(())
        }
        Cmd::Remove { package, purge } => cmd_remove(&package, &bin_dir, assume_yes, purge),
        Cmd::Update { package, all } => {
            cmd_update(package.as_deref(), all, &bin_dir, track, no_hooks, assume_yes, dry_run)
        }
        Cmd::List { installed } => cmd_list(installed, track),
        Cmd::Info { package } => cmd_info(&package, track),
        Cmd::Search { query } => cmd_search(&query, track),
        Cmd::Doctor { package } => cmd_doctor(package.as_deref(), track, &bin_dir),
        Cmd::Verify { package } => cmd_verify(package.as_deref(), &bin_dir),
        Cmd::Rollback { package } => cmd_rollback(&package, &bin_dir),
        // Same update logic as `bakery update bakery` — this is just a
        // documented, discoverable entry point for it, since overwriting
        // bakery's own running binary via a normal update already works
        // (rename-over-running-binary is safe on Linux) but wasn't a real
        // first-class command.
        Cmd::SelfUpdate => cmd_update(Some("bakery"), false, &bin_dir, track, no_hooks, assume_yes, dry_run),
        Cmd::Completions { shell } => cmd_completions(shell),
        Cmd::Track { action } => cmd_track(action),
    }
}

fn cmd_completions(shell: clap_complete::Shell) -> Result<()> {
    clap_complete::generate(shell, &mut Cli::command(), "bakery", &mut std::io::stdout());
    Ok(())
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
    dry_run: bool,
) -> Result<()> {
    let mut visited = HashSet::new();
    install_with_deps(index, name, bin_dir, track, no_hooks, assume_yes, dry_run, &mut visited)
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
    dry_run: bool,
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
            println!("{} bread dependency: {dep}", if dry_run { "would install" } else { "installing" });
            install_with_deps(index, &dep, bin_dir, track, no_hooks, assume_yes, dry_run, visited)?;
        }
    }

    let previous = state.packages.get(name);

    // Already installed and not older than the index — nothing to do. This
    // doubles as the implicit upgrade path when the index has something
    // newer, so `bakery install <pkg>` is safe to run repeatedly instead of
    // silently reinstalling (and potentially downgrading) every time.
    if let Some(installed) = previous {
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

    if dry_run {
        print_dry_run_plan(pkg, previous);
        return Ok(());
    }

    install::install_package(pkg, bin_dir, track, previous, no_hooks, assume_yes)
}

/// Prints what `install_with_deps`/`cmd_update` would do for `pkg` under
/// `--dry-run`, once the version-comparison decision to actually act has
/// already been made — this only renders that decision, it never
/// recomputes it, so dry-run and real runs can't drift apart on "would this
/// update happen at all".
fn print_dry_run_plan(pkg: &manifest::Package, previous: Option<&state::InstalledPackage>) {
    let verb = if previous.is_some() { "update" } else { "install" };
    println!(
        "  {} would {verb} {} to {}",
        ui::style("dry-run:", ui::DIM),
        pkg.name,
        ui::style(&pkg.version, ui::BOLD)
    );
    println!(
        "    binaries: {}",
        pkg.binaries.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")
    );
    if !pkg.services.is_empty() {
        println!(
            "    services: {}",
            pkg.services.iter().map(|s| s.unit.as_str()).collect::<Vec<_>>().join(", ")
        );
    }
}

fn cmd_remove(name: &str, bin_dir: &std::path::Path, assume_yes: bool, purge: bool) -> Result<()> {
    install::remove_package(name, bin_dir, assume_yes, purge)
}

#[allow(clippy::too_many_arguments)]
fn cmd_update(
    name: Option<&str>,
    all: bool,
    bin_dir: &std::path::Path,
    track: Track,
    no_hooks: bool,
    assume_yes: bool,
    dry_run: bool,
) -> Result<()> {
    let index = manifest::load(true, track)?;
    let state = state::State::load()?;

    let targets: Vec<String> = match name {
        Some(n) if !all => vec![n.to_string()],
        _ => state.packages.keys().cloned().collect(),
    };

    if targets.is_empty() {
        println!("no packages installed");
        return Ok(());
    }

    let mut any_failed = false;
    let mut updated = 0u32;
    let mut unchanged = 0u32;
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
            // DIM, not GREEN — this is the steady-state common case (most
            // packages, most runs), and reusing GREEN here drowns out the
            // packages that actually changed below. The glyph itself
            // (rather than just color) keeps that distinction legible even
            // under a terminal palette that maps ANSI colors unusually.
            println!(
                "  {}",
                ui::unchanged(&format!("{pkg_name} is already at {}", installed.version))
            );
            unchanged += 1;
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

        if dry_run {
            print_dry_run_plan(latest, Some(installed));
            updated += 1;
            continue;
        }

        if let Err(e) = install::install_package(latest, bin_dir, track, Some(installed), no_hooks, assume_yes) {
            eprintln!("  failed to update {pkg_name}: {e}");
            any_failed = true;
        } else {
            updated += 1;
        }
    }

    // Only for --all: a single named update already makes its own outcome
    // obvious, and "1 updated, 0 already up to date" isn't a useful takeaway.
    if all {
        let mut parts = Vec::new();
        if updated > 0 {
            parts.push(format!("{updated} updated"));
        }
        parts.push(format!("{unchanged} already up to date"));
        println!("{}", ui::style(&parts.join(", "), ui::BOLD));
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

/// Prints one index entry in the shared `list`/`search` format: name,
/// version, description, and an `[installed <version>]` tag when applicable.
fn print_index_entry(pkg: &manifest::Package, state: &state::State) {
    let tag = if state.is_installed(&pkg.name) {
        ui::style(&format!(" [installed {}]", state.packages[&pkg.name].version), ui::GREEN)
    } else {
        String::new()
    };
    println!("  {:<14} {:<10} — {}{}", pkg.name, pkg.version, pkg.description, tag);
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
        print_index_entry(&index.packages[name], &state);
    }
    Ok(())
}

/// Case-insensitive substring match against a package's name or description
/// — split out from `cmd_search` so the matching rule itself is testable
/// without a real index load.
fn matches_search(name: &str, description: &str, needle_lower: &str) -> bool {
    name.to_lowercase().contains(needle_lower) || description.to_lowercase().contains(needle_lower)
}

fn cmd_search(query: &str, track: Track) -> Result<()> {
    let state = state::State::load()?;
    let index = manifest::load(false, track)?;
    let needle = query.to_lowercase();

    let mut names: Vec<&str> = index
        .packages
        .values()
        .filter(|pkg| matches_search(&pkg.name, &pkg.description, &needle))
        .map(|pkg| pkg.name.as_str())
        .collect();
    names.sort();

    if names.is_empty() {
        println!("no packages matched '{query}'");
        return Ok(());
    }

    for name in names {
        print_index_entry(&index.packages[name], &state);
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
        // only, not a checksum re-verification — see `bakery verify` for that.
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

#[derive(Debug, PartialEq, Eq)]
enum VerifyStatus {
    Ok,
    Missing,
    Tampered,
    /// `binary_sha256` has no entry for this binary — an install that
    /// predates `bakery verify` support. Reported plainly rather than
    /// folded into `Ok`, since there's nothing to actually compare against.
    Unknown,
}

/// Recomputes `binary_name`'s on-disk sha256 in `bin_dir` and compares it
/// against `expected` — the hash recorded in `InstalledPackage::
/// binary_sha256` at install time, not a fresh index lookup. The index
/// only carries the checksum for whatever the *current latest* release is,
/// which may not be what's actually installed if the user hasn't updated
/// yet; comparing against that would produce a false "tampered" report for
/// a perfectly intact, just-not-latest binary.
fn verify_binary(bin_dir: &Path, binary_name: &str, expected: Option<&String>) -> VerifyStatus {
    let Some(expected) = expected else {
        return VerifyStatus::Unknown;
    };
    let path = bin_dir.join(binary_name);
    let Ok(bytes) = std::fs::read(&path) else {
        return VerifyStatus::Missing;
    };
    let actual = hex::encode(Sha256::digest(&bytes));
    if &actual == expected {
        VerifyStatus::Ok
    } else {
        VerifyStatus::Tampered
    }
}

fn cmd_verify(name: Option<&str>, bin_dir: &std::path::Path) -> Result<()> {
    let state = state::State::load()?;

    let targets: Vec<String> = match name {
        Some(n) => {
            if !state.is_installed(n) {
                bail!("{n} is not installed");
            }
            vec![n.to_string()]
        }
        None => state.packages.keys().cloned().collect(),
    };

    if targets.is_empty() {
        println!("no packages installed — nothing to verify");
        return Ok(());
    }

    let mut any_bad = false;
    for pkg_name in &targets {
        let installed = &state.packages[pkg_name];
        if installed.binary_sha256.is_empty() {
            println!(
                "  {} {pkg_name}: no recorded checksums (installed before 'bakery verify' support)",
                ui::style("?", ui::DIM)
            );
            continue;
        }
        for bin in &installed.binaries {
            match verify_binary(bin_dir, bin, installed.binary_sha256.get(bin)) {
                VerifyStatus::Ok => println!("  {}", ui::ok(&format!("{pkg_name}: {bin}"))),
                VerifyStatus::Missing => {
                    eprintln!("  {}", ui::fail(&format!("{pkg_name}: {bin} — MISSING")));
                    any_bad = true;
                }
                VerifyStatus::Tampered => {
                    eprintln!(
                        "  {}",
                        ui::fail(&format!("{pkg_name}: {bin} — TAMPERED (checksum mismatch)"))
                    );
                    any_bad = true;
                }
                VerifyStatus::Unknown => {
                    println!(
                        "  {} {pkg_name}: {bin} — UNKNOWN (no recorded checksum for this binary)",
                        ui::style("?", ui::DIM)
                    );
                }
            }
        }
    }

    if any_bad {
        bail!("verification failed for one or more binaries");
    }
    println!("{}", ui::ok("all recorded checksums match"));
    Ok(())
}

/// Copies each of `binaries` from `backup_dir` into `bin_dir` (atomic,
/// executable — same as a normal install), returning the sha256 of each
/// restored binary so the caller can update `InstalledPackage::
/// binary_sha256` to match what's now actually on disk. The hash is
/// computed from the trusted local backup bytes directly, not re-verified
/// against any network source — see `install::backup_current_binary` for
/// why rollback is backup-based rather than a network re-pin in the first
/// place. Pure with respect to global state (caller supplies both dirs), so
/// this is the piece of `bakery rollback` that's directly unit-testable.
fn restore_binaries(backup_dir: &Path, binaries: &[String], bin_dir: &Path) -> Result<HashMap<String, String>> {
    let mut sha256 = HashMap::new();
    for bin in binaries {
        let backup_path = backup_dir.join(bin);
        if !backup_path.exists() {
            bail!("backup for binary '{bin}' is missing at {}", backup_path.display());
        }
        let bytes = std::fs::read(&backup_path)
            .with_context(|| format!("reading backup {}", backup_path.display()))?;
        let hash = hex::encode(Sha256::digest(&bytes));
        let dest = bin_dir.join(bin);
        bread_utils::atomic::write_atomic_bytes(&dest, &bytes, Some(0o755))
            .with_context(|| format!("restoring {}", dest.display()))?;
        sha256.insert(bin.clone(), hash);
    }
    Ok(sha256)
}

/// Rolls `pkg_name` back to its previously installed version, restoring
/// binaries from the local pre-update backup `install::install_package`
/// made — not by re-fetching the old version from `dl.breadway.dev`.
/// Deliberately backup-based: `index.json`'s minisign signature only covers
/// the *current* published version's checksums, so verifying an old version
/// pulled fresh from the server would only be checkable against its
/// unsigned per-version `.sha256` sidecar file, a materially weaker
/// guarantee than bakery's normal trust model. The local backup sidesteps
/// that gap — it's bytes bakery itself copied from a binary that was
/// already verified against a signed index at the time it was installed.
fn cmd_rollback(pkg_name: &str, bin_dir: &std::path::Path) -> Result<()> {
    let installed = state::State::load()?
        .packages
        .get(pkg_name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{pkg_name} is not installed"))?;

    let target_version = installed.previous_version.clone().ok_or_else(|| {
        anyhow::anyhow!("no previous version recorded for {pkg_name} — nothing to roll back to")
    })?;

    let backup_dir = state::backup_dir(pkg_name, &target_version);
    if !backup_dir.exists() {
        bail!(
            "no local backup found for {pkg_name} {target_version} at {} — cannot roll back",
            backup_dir.display()
        );
    }

    let binary_sha256 = restore_binaries(&backup_dir, &installed.binaries, bin_dir)?;

    let from_version = installed.version.clone();
    state::State::with_lock(|state| {
        if let Some(pkg) = state.packages.get_mut(pkg_name) {
            pkg.version = target_version.clone();
            pkg.previous_version = Some(from_version.clone());
            pkg.binary_sha256 = binary_sha256.clone();
        }
        Ok(())
    })?;

    // Best-effort — a leftover backup dir after a successful rollback just
    // wastes disk, it's not a correctness problem, so a failure here
    // shouldn't fail the rollback itself.
    let _ = std::fs::remove_dir_all(&backup_dir);

    println!(
        "  {}",
        ui::ok(&format!("rolled back {pkg_name} {from_version} → {target_version}"))
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

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

    #[test]
    fn matches_search_matches_name_case_insensitively() {
        assert!(matches_search("BreadHelp", "onboarding guide", "breadhelp"));
    }

    #[test]
    fn matches_search_matches_description_substring() {
        assert!(matches_search("breadhelp", "Onboarding Guide", "onboard"));
    }

    #[test]
    fn matches_search_no_match_returns_false() {
        assert!(!matches_search("breadhelp", "onboarding guide", "zzz"));
    }

    fn empty_binary_package(name: &str, version: &str, url: &str) -> manifest::Package {
        manifest::Package {
            name: name.to_string(),
            description: "test".to_string(),
            version: version.to_string(),
            binaries: vec![manifest::Binary {
                name: name.to_string(),
                dl_url: url.to_string(),
                github_url: url.to_string(),
                sha256: "0".repeat(64),
            }],
            system_deps: vec![],
            optional_system_deps: vec![],
            bread_deps: vec![],
            services: vec![],
            config: None,
            post_install: vec![],
            license_file: None,
            license_file_sha256: None,
            desktop_file: None,
            desktop_file_sha256: None,
            data_archive: None,
            data_archive_sha256: None,
        }
    }

    #[test]
    fn install_with_deps_dry_run_does_not_download_or_write() {
        let name = "faketestpkg-dry-run";
        // A reserved, essentially-guaranteed-unreachable port: if dry-run
        // ever regressed into actually calling fetch_and_place, this would
        // fail loudly (connection refused) instead of silently passing.
        let pkg = empty_binary_package(name, "9.9.9", "http://127.0.0.1:1/unreachable");
        let mut packages = std::collections::HashMap::new();
        packages.insert(name.to_string(), pkg);
        let index = manifest::Index { version: "1".to_string(), packages };

        let bin_dir = tempdir().unwrap();
        let mut visited = HashSet::new();
        install_with_deps(
            &index,
            name,
            bin_dir.path(),
            Track::Stable,
            true,
            true,
            true, // dry_run
            &mut visited,
        )
        .unwrap();

        assert!(fs::read_dir(bin_dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn verify_binary_ok_when_hash_matches() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("mypkg"), b"good bytes").unwrap();
        let hash = hex::encode(Sha256::digest(b"good bytes"));
        assert_eq!(verify_binary(dir.path(), "mypkg", Some(&hash)), VerifyStatus::Ok);
    }

    #[test]
    fn verify_binary_tampered_when_hash_mismatches() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("mypkg"), b"tampered bytes").unwrap();
        let wrong_hash = "0".repeat(64);
        assert_eq!(verify_binary(dir.path(), "mypkg", Some(&wrong_hash)), VerifyStatus::Tampered);
    }

    #[test]
    fn verify_binary_missing_when_file_absent() {
        let dir = tempdir().unwrap();
        let hash = "0".repeat(64);
        assert_eq!(verify_binary(dir.path(), "nope", Some(&hash)), VerifyStatus::Missing);
    }

    #[test]
    fn verify_binary_unknown_when_no_recorded_hash() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("mypkg"), b"bytes").unwrap();
        assert_eq!(verify_binary(dir.path(), "mypkg", None), VerifyStatus::Unknown);
    }

    #[test]
    fn restore_binaries_round_trips_backup_into_bin_dir() {
        let dir = tempdir().unwrap();
        let backup_dir = dir.path().join("backup");
        fs::create_dir_all(&backup_dir).unwrap();
        fs::write(backup_dir.join("mypkg"), b"old version bytes").unwrap();
        let bin_dir = dir.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let hashes = restore_binaries(&backup_dir, &["mypkg".to_string()], &bin_dir).unwrap();

        assert_eq!(fs::read(bin_dir.join("mypkg")).unwrap(), b"old version bytes");
        assert_eq!(hashes["mypkg"], hex::encode(Sha256::digest(b"old version bytes")));
    }

    #[test]
    fn restore_binaries_errors_clearly_when_backup_missing() {
        let dir = tempdir().unwrap();
        let backup_dir = dir.path().join("backup");
        let bin_dir = dir.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let err = restore_binaries(&backup_dir, &["mypkg".to_string()], &bin_dir).unwrap_err();

        assert!(err.to_string().contains("missing"));
        assert!(fs::read_dir(&bin_dir).unwrap().next().is_none());
    }
}
