mod doctor;
mod download;
mod install;
mod manifest;
mod state;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "bakery", about = "Package manager for the bread ecosystem", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
    /// Override the directory where binaries are installed
    #[arg(long, env = "BAKERY_BIN_DIR", global = true)]
    bin_dir: Option<PathBuf>,
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
}

fn default_bin_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".local/bin")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let bin_dir = cli.bin_dir.unwrap_or_else(default_bin_dir);

    match cli.command {
        Cmd::Install { packages } => {
            let index = manifest::load(true)?;
            for pkg in &packages {
                cmd_install(&index, pkg, &bin_dir)?;
            }
            Ok(())
        }
        Cmd::Remove { package } => cmd_remove(&package, &bin_dir),
        Cmd::Update { package, all } => cmd_update(package.as_deref(), all, &bin_dir),
        Cmd::List { installed } => cmd_list(installed),
        Cmd::Info { package } => cmd_info(&package),
        Cmd::Doctor { package } => cmd_doctor(package.as_deref()),
    }
}

fn cmd_install(index: &manifest::Index, name: &str, bin_dir: &std::path::Path) -> Result<()> {
    let mut visited = HashSet::new();
    install_with_deps(index, name, bin_dir, &mut visited)
}

/// Recursively installs `name` and any bread_deps, skipping already-installed
/// packages. The `visited` set prevents cycles.
fn install_with_deps(
    index: &manifest::Index,
    name: &str,
    bin_dir: &std::path::Path,
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
            install_with_deps(index, &dep, bin_dir, visited)?;
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

    install::install_package(pkg, bin_dir)
}

fn cmd_remove(name: &str, bin_dir: &std::path::Path) -> Result<()> {
    install::remove_package(name, bin_dir)
}

fn cmd_update(name: Option<&str>, all: bool, bin_dir: &std::path::Path) -> Result<()> {
    let index = manifest::load(true)?;
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
                continue;
            }
        };
        let latest = match index.get(pkg_name) {
            Some(p) => p,
            None => {
                eprintln!("{pkg_name} not found in index, skipping");
                continue;
            }
        };

        if installed.version == latest.version {
            println!("{pkg_name} is already at {}", installed.version);
            continue;
        }

        println!(
            "updating {pkg_name} {} → {}",
            installed.version, latest.version
        );

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

        if let Err(e) = install::install_package(latest, bin_dir) {
            eprintln!("  failed to update {pkg_name}: {e}");
            any_failed = true;
        }
    }

    if any_failed {
        bail!("one or more packages could not be updated");
    }
    Ok(())
}

fn cmd_list(installed_only: bool) -> Result<()> {
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

    let index = manifest::load(false)?;
    let mut names: Vec<&str> = index.packages.keys().map(|s| s.as_str()).collect();
    names.sort();
    for name in names {
        let pkg = &index.packages[name];
        let tag = if state.is_installed(name) {
            format!(" [installed {}]", state.packages[name].version)
        } else {
            String::new()
        };
        println!("  {} {} — {}{}", pkg.name, pkg.version, pkg.description, tag);
    }
    Ok(())
}

fn cmd_info(name: &str) -> Result<()> {
    let index = manifest::load(false)?;
    let pkg = index
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("unknown package: {name}"))?;

    let state = state::State::load()?;
    let status = if let Some(inst) = state.packages.get(name) {
        format!("installed ({})", inst.version)
    } else {
        "not installed".to_string()
    };

    println!("{} {}", pkg.name, pkg.version);
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

fn cmd_doctor(name: Option<&str>) -> Result<()> {
    let index = manifest::load(false)?;
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
    }

    if all_ok {
        println!("all checks passed");
    }
    Ok(())
}
