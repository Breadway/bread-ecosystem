use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::download::fetch_and_place;
use crate::manifest::{fetch_binary, Package, Service};
use crate::state::{InstalledPackage, State};

pub fn install_package(pkg: &Package, bin_dir: &Path) -> Result<()> {
    println!("installing {}@{}…", pkg.name, pkg.version);

    // 1. Download and verify all binaries.
    let mut binary_names = Vec::new();
    for bin in &pkg.binaries {
        let install_name = strip_arch_suffix(&bin.name);
        let dest = bin_dir.join(&install_name);
        fetch_and_place(bin, &dest)?;
        binary_names.push(install_name.to_string());
    }

    // 2. Scaffold config dir + download example file.
    if let Some(cfg) = &pkg.config {
        scaffold_config(cfg, pkg)?;
    }

    // 3. Install systemd user units.
    let mut service_names = Vec::new();
    for svc in &pkg.services {
        install_service(svc, bin_dir, pkg)?;
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
    // Commit removal immediately — file cleanup below is best-effort.
    state.save()?;

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

    println!("  {pkg_name} removed");
    Ok(())
}

fn scaffold_config(cfg: &crate::manifest::ConfigScaffold, pkg: &Package) -> Result<()> {
    let dir = expand_tilde(&cfg.dir);
    std::fs::create_dir_all(&dir)?;

    if let Some(example) = &cfg.example {
        let dest = dir.join(example);
        if !dest.exists() {
            if let Some((primary, fallback)) = pkg.artifact_urls(example) {
                match fetch_binary(&primary, &fallback) {
                    Ok(bytes) => {
                        std::fs::write(&dest, &bytes)
                            .with_context(|| format!("writing {}", dest.display()))?;
                        println!("  installed example config at {}", dest.display());
                    }
                    Err(e) => {
                        eprintln!("  warning: could not download example config {example}: {e}");
                        println!("  config dir created at {}", dir.display());
                    }
                }
            } else {
                println!("  config dir created at {}", dir.display());
            }
        } else {
            println!("  config at {} already exists, skipping", dest.display());
        }
    } else {
        println!("  config dir created at {}", dir.display());
    }
    Ok(())
}

fn install_service(svc: &Service, bin_dir: &Path, pkg: &Package) -> Result<()> {
    let service_dir = systemd_user_dir();
    std::fs::create_dir_all(&service_dir)?;

    let unit_path = service_dir.join(&svc.unit);

    // Download the unit file if not already present.
    if !unit_path.exists() {
        if let Some((primary, fallback)) = pkg.artifact_urls(&svc.unit) {
            match fetch_binary(&primary, &fallback) {
                Ok(bytes) => {
                    std::fs::write(&unit_path, &bytes)
                        .with_context(|| format!("writing {}", unit_path.display()))?;
                    println!("  downloaded unit {}", unit_path.display());
                }
                Err(e) => {
                    eprintln!("  warning: could not download {}: {e}", svc.unit);
                }
            }
        } else {
            eprintln!("  warning: no artifact URL to download {}", svc.unit);
        }
    }

    if !unit_path.exists() {
        eprintln!(
            "  warning: unit file {} not found — skipping service setup",
            svc.unit
        );
        return Ok(());
    }

    patch_exec_start(&unit_path, bin_dir)?;

    if !Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        eprintln!("  warning: systemctl daemon-reload failed");
    }

    if svc.enable {
        let already_active = Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", &svc.unit])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if already_active {
            if Command::new("systemctl")
                .args(["--user", "restart", &svc.unit])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
            {
                println!("  {} restarted", svc.unit);
            } else {
                eprintln!("  warning: failed to restart {}", svc.unit);
            }
        } else if Command::new("systemctl")
            .args(["--user", "enable", "--now", &svc.unit])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            println!("  {} enabled and started", svc.unit);
        } else {
            eprintln!("  warning: failed to enable {}", svc.unit);
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
    // Preserve trailing newline if the original had one.
    let output = if text.ends_with('\n') {
        format!("{patched}\n")
    } else {
        patched
    };
    std::fs::write(unit_path, output)?;
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

pub fn strip_arch_suffix(name: &str) -> &str {
    const SUFFIXES: &[&str] = &["-x86_64", "-aarch64", "-arm64", "-armv7"];
    for s in SUFFIXES {
        if let Some(base) = name.strip_suffix(s) {
            return base;
        }
    }
    name
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn strip_known_suffixes() {
        assert_eq!(strip_arch_suffix("breadd-x86_64"), "breadd");
        assert_eq!(strip_arch_suffix("breadd-aarch64"), "breadd");
        assert_eq!(strip_arch_suffix("breadd-arm64"), "breadd");
        assert_eq!(strip_arch_suffix("breadd-armv7"), "breadd");
        assert_eq!(strip_arch_suffix("bakery-x86_64"), "bakery");
        assert_eq!(strip_arch_suffix("breadd"), "breadd");
    }

    #[test]
    fn patch_exec_start_with_args() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.service");
        fs::write(&path, "[Service]\nExecStart=/old/path/bin arg1 arg2\n").unwrap();
        patch_exec_start(&path, Path::new("/new/bin")).unwrap();
        let out = fs::read_to_string(&path).unwrap();
        assert!(out.contains("ExecStart=/new/bin/bin arg1 arg2"));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn patch_exec_start_no_args() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.service");
        fs::write(&path, "[Service]\nExecStart=/old/path/daemon\n").unwrap();
        patch_exec_start(&path, Path::new("/usr/local/bin")).unwrap();
        let out = fs::read_to_string(&path).unwrap();
        assert!(out.contains("ExecStart=/usr/local/bin/daemon"));
        assert!(!out.contains("daemon "));
    }

    #[test]
    fn patch_exec_start_non_exec_lines_unchanged() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.service");
        fs::write(&path, "[Unit]\nDescription=foo\nExecStart=/bin/foo\n").unwrap();
        patch_exec_start(&path, Path::new("/usr/bin")).unwrap();
        let out = fs::read_to_string(&path).unwrap();
        assert!(out.contains("Description=foo"));
        assert!(out.contains("ExecStart=/usr/bin/foo"));
    }
}
