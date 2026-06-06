use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::download::fetch_and_place;
use crate::manifest::{Package, Service};
use crate::state::{InstalledPackage, State};

pub fn install_package(pkg: &Package, bin_dir: &Path) -> Result<()> {
    println!("installing {}@{}…", pkg.name, pkg.version);

    // 1. Download and verify all binaries.
    let mut binary_names = Vec::new();
    for bin in &pkg.binaries {
        let dest = bin_dir.join(&bin.name);
        fetch_and_place(bin, &dest)?;
        binary_names.push(bin.name.clone());
    }

    // 2. Scaffold config dir + example file.
    if let Some(cfg) = &pkg.config {
        scaffold_config(cfg)?;
    }

    // 3. Install systemd user units.
    let mut service_names = Vec::new();
    for svc in &pkg.services {
        install_service(svc, bin_dir)?;
        service_names.push(svc.unit.clone());
    }

    // 4. Run post_install hooks.
    for cmd in &pkg.post_install {
        run_hook(cmd, &pkg.name)?;
    }

    // 5. Record in state.
    let mut state = State::load()?;
    state.record(InstalledPackage {
        name: pkg.name.clone(),
        version: pkg.version.clone(),
        binaries: binary_names,
        services: service_names,
        installed_at: chrono::Utc::now().to_rfc3339(),
    });
    state.save()?;

    println!("  {} installed successfully", pkg.name);
    warn_path_if_needed(bin_dir);
    Ok(())
}

pub fn remove_package(pkg_name: &str, bin_dir: &Path) -> Result<()> {
    let mut state = State::load()?;
    let installed = match state.remove(pkg_name) {
        Some(p) => p,
        None => {
            eprintln!("{pkg_name} is not installed");
            return Ok(());
        }
    };

    // Remove binaries.
    for bin in &installed.binaries {
        let path = bin_dir.join(bin);
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("removing {}", path.display()))?;
            println!("  removed {}", path.display());
        }
    }

    // Prompt for unit removal.
    if !installed.services.is_empty() {
        let service_dir = systemd_user_dir();
        for unit in &installed.services {
            let unit_path = service_dir.join(unit);
            if confirm_remove_unit(unit) {
                let _ = Command::new("systemctl")
                    .args(["--user", "disable", "--now", unit])
                    .status();
                if unit_path.exists() {
                    std::fs::remove_file(&unit_path).ok();
                }
                let _ = Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .status();
                println!("  removed unit {unit}");
            }
        }
    }

    // Never touch config or data dirs.
    if let Some(cfg_dir) = guess_config_dir(pkg_name) {
        if cfg_dir.exists() {
            println!("  config preserved at {}", cfg_dir.display());
        }
    }
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join(pkg_name);
    if data_dir.exists() {
        println!("  data preserved at {}", data_dir.display());
    }

    state.save()?;
    println!("  {pkg_name} removed");
    Ok(())
}

fn scaffold_config(cfg: &crate::manifest::ConfigScaffold) -> Result<()> {
    let dir = expand_tilde(&cfg.dir);
    std::fs::create_dir_all(&dir)?;
    if let Some(example) = &cfg.example {
        let dest = dir.join(example);
        if !dest.exists() {
            // We don't have the actual example file here at install time —
            // the product repo's release bundle should include it.
            // For now just note it; release.yml will bundle example configs.
            println!("  config dir ready at {}", dir.display());
            println!(
                "  copy your {example} to {} to configure {}",
                dest.display(),
                dir.display()
            );
        } else {
            println!("  config at {} already exists, skipping", dest.display());
        }
    }
    Ok(())
}

fn install_service(svc: &Service, bin_dir: &Path) -> Result<()> {
    let service_dir = systemd_user_dir();
    std::fs::create_dir_all(&service_dir)?;

    let unit_path = service_dir.join(&svc.unit);

    // The unit file is expected to be bundled alongside the binary in the
    // release artifact (or embedded). For now, patch ExecStart if the unit
    // already exists (same pattern as bread/scripts/install.sh).
    if unit_path.exists() {
        patch_exec_start(&unit_path, bin_dir)?;
    }

    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();

    if svc.enable {
        if Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", &svc.unit])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            let _ = Command::new("systemctl")
                .args(["--user", "restart", &svc.unit])
                .status();
            println!("  {} restarted", svc.unit);
        } else {
            let _ = Command::new("systemctl")
                .args(["--user", "enable", "--now", &svc.unit])
                .status();
            println!("  {} enabled and started", svc.unit);
        }
    }

    Ok(())
}

fn patch_exec_start(unit_path: &Path, bin_dir: &Path) -> Result<()> {
    let text = std::fs::read_to_string(unit_path)?;
    let patched: String = text
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("ExecStart=") {
                // Replace only the path prefix, keep args.
                let rest = line.splitn(2, '=').nth(1).unwrap_or("");
                let argv: Vec<&str> = rest.split_whitespace().collect();
                if let Some(bin_name) = argv.first().and_then(|p| Path::new(p).file_name()) {
                    let new_path = bin_dir.join(bin_name);
                    let args: Vec<&str> = argv.iter().skip(1).copied().collect();
                    if args.is_empty() {
                        format!("ExecStart={}", new_path.display())
                    } else {
                        format!("ExecStart={} {}", new_path.display(), args.join(" "))
                    }
                } else {
                    line.to_string()
                }
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(unit_path, patched)?;
    Ok(())
}

fn run_hook(cmd: &str, pkg_name: &str) -> Result<()> {
    println!("  running post_install hook: {cmd}");
    let status = Command::new("sh")
        .args(["-c", cmd])
        .status()
        .with_context(|| format!("running post_install hook for {pkg_name}"))?;
    if !status.success() {
        eprintln!("  warning: hook exited with {status}");
    }
    Ok(())
}

fn confirm_remove_unit(unit: &str) -> bool {
    use std::io::{self, Write};
    print!("  remove systemd unit {unit}? [y/N] ");
    io::stdout().flush().ok();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf).ok();
    matches!(buf.trim().to_lowercase().as_str(), "y" | "yes")
}

fn systemd_user_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("systemd/user")
}

fn guess_config_dir(pkg_name: &str) -> Option<PathBuf> {
    Some(dirs::config_dir()?.join(pkg_name))
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(rest)
    } else {
        PathBuf::from(path)
    }
}

fn warn_path_if_needed(bin_dir: &Path) {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let bin_str = bin_dir.to_string_lossy();
    if !path_var.split(':').any(|p| p == bin_str) {
        println!(
            "\n  note: {} is not in PATH — add to your shell profile:",
            bin_str
        );
        println!("    export PATH=\"{}:$PATH\"", bin_str);
    }
}
