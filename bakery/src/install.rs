use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::download::{fetch_and_place, verify_sha256};
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

    // 3. Install license file, if declared.
    if let Some(license) = &pkg.license_file {
        install_license(pkg, license)?;
    }

    // 4. Install desktop entry, if declared.
    if let Some(desktop) = &pkg.desktop_file {
        install_desktop_file(pkg, desktop)?;
    }

    // 5. Download + extract data archive, if declared.
    if let Some(archive) = &pkg.data_archive {
        install_data_archive(pkg, archive)?;
    }

    // 6. Install systemd user units.
    let mut service_names = Vec::new();
    for svc in &pkg.services {
        install_service(svc, bin_dir, pkg)?;
        service_names.push(svc.unit.clone());
    }

    // 7. Run post_install hooks.
    for cmd in &pkg.post_install {
        run_hook(cmd, &pkg.name)?;
    }

    // 8. Record in state.
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
                    Ok(bytes) => match &cfg.example_sha256 {
                        Some(expected) => match verify_sha256(&bytes, expected) {
                            Ok(()) => {
                                std::fs::write(&dest, &bytes)
                                    .with_context(|| format!("writing {}", dest.display()))?;
                                println!("  installed example config at {}", dest.display());
                            }
                            Err(e) => {
                                eprintln!(
                                    "  warning: checksum mismatch for example config {example}: {e} — not installed"
                                );
                                println!("  config dir created at {}", dir.display());
                            }
                        },
                        None => {
                            eprintln!(
                                "  warning: index.json has no sha256 for example config \
                                 {example} — refusing to install an unverified download"
                            );
                            println!("  config dir created at {}", dir.display());
                        }
                    },
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

/// Download `filename` from `pkg`'s release dir, verify it against `sha256`
/// (refusing an unverified download the same way `scaffold_config` does),
/// and write it to `dest`. Shared by `install_license`/`install_desktop_file`
/// since both are "fetch one small artifact, verify, place" — unlike a config
/// example, these aren't user-editable, so they're always refreshed rather
/// than skipped when already present.
fn fetch_verify_write(
    pkg: &Package,
    filename: &str,
    sha256: &Option<String>,
    dest: &Path,
    label: &str,
) -> Result<()> {
    let Some((primary, fallback)) = pkg.artifact_urls(filename) else {
        eprintln!("  warning: no artifact URL to download {label} ({filename})");
        return Ok(());
    };
    let bytes = match fetch_binary(&primary, &fallback) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("  warning: could not download {label} {filename}: {e}");
            return Ok(());
        }
    };
    let Some(expected) = sha256 else {
        eprintln!(
            "  warning: index.json has no sha256 for {label} {filename} — \
             refusing to install an unverified download"
        );
        return Ok(());
    };
    if let Err(e) = verify_sha256(&bytes, expected) {
        eprintln!("  warning: checksum mismatch for {label} {filename}: {e} — not installed");
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, &bytes).with_context(|| format!("writing {}", dest.display()))?;
    println!("  installed {label} at {}", dest.display());
    Ok(())
}

fn install_license(pkg: &Package, filename: &str) -> Result<()> {
    let dest = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("licenses")
        .join(&pkg.name)
        .join("LICENSE");
    fetch_verify_write(pkg, filename, &pkg.license_file_sha256, &dest, "license")
}

fn install_desktop_file(pkg: &Package, filename: &str) -> Result<()> {
    let dest = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("applications")
        .join(format!("{}.desktop", pkg.name));
    fetch_verify_write(pkg, filename, &pkg.desktop_file_sha256, &dest, "desktop entry")
}

fn install_data_archive(pkg: &Package, filename: &str) -> Result<()> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join(&pkg.name);
    fetch_extract_archive(pkg, filename, &pkg.data_archive_sha256, &data_dir)
}

/// Downloads + verifies a `.tar.gz` artifact, then extracts it into
/// `dest_dir`. Shells out to `tar` rather than adding an archive-extraction
/// crate dependency — `tar` is universally present on Linux and this file
/// already shells out to `systemctl` for the same "trust the base system
/// has this" reason. Split from `install_data_archive` (which just supplies
/// the real `~/.local/share/<name>` destination) so tests can extract into
/// a tempdir instead.
fn fetch_extract_archive(
    pkg: &Package,
    filename: &str,
    sha256: &Option<String>,
    dest_dir: &Path,
) -> Result<()> {
    let tmp_archive = std::env::temp_dir().join(format!("bakery-{}-{filename}", pkg.name));

    fetch_verify_write(pkg, filename, sha256, &tmp_archive, "data archive")?;
    if !tmp_archive.exists() {
        // fetch_verify_write already warned (download/checksum failure).
        return Ok(());
    }

    std::fs::create_dir_all(dest_dir)?;
    let status = Command::new("tar")
        .args(["xzf", &tmp_archive.to_string_lossy(), "-C"])
        .arg(dest_dir)
        .status()
        .with_context(|| format!("running tar to extract {filename}"))?;
    let _ = std::fs::remove_file(&tmp_archive);

    if status.success() {
        println!("  extracted {filename} to {}", dest_dir.display());
    } else {
        eprintln!("  warning: tar exited with {status} extracting {filename}");
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
                Ok(bytes) => match verify_sha256(&bytes, &svc.sha256) {
                    Ok(()) => {
                        std::fs::write(&unit_path, &bytes)
                            .with_context(|| format!("writing {}", unit_path.display()))?;
                        println!("  downloaded unit {}", unit_path.display());
                    }
                    Err(e) => {
                        eprintln!(
                            "  warning: checksum mismatch for unit {}: {e} — not installed",
                            svc.unit
                        );
                    }
                },
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
    use crate::manifest::{Binary, Package};
    use sha2::Digest;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use tempfile::tempdir;

    /// Serves `body` for exactly one HTTP/1.0 request on an ephemeral local
    /// port, then stops. Real network I/O over loopback — exercises
    /// `fetch_verify_write`'s actual `fetch_binary` call, not just its
    /// surrounding logic, without any new test dependency.
    fn serve_once(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        format!("http://{addr}")
    }

    /// Same as `serve_once` but for a runtime-owned body (e.g. a tar.gz
    /// built into a tempdir during the test), which can't satisfy `'static`.
    fn serve_once_owned(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://{addr}")
    }

    fn test_package(binary_url: &str) -> Package {
        Package {
            name: "breadhelp".to_string(),
            description: "test".to_string(),
            version: "1.0.0".to_string(),
            binaries: vec![Binary {
                name: "breadhelp-x86_64".to_string(),
                dl_url: format!("{binary_url}/breadhelp-x86_64"),
                github_url: format!("{binary_url}/breadhelp-x86_64"),
                sha256: String::new(),
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
    fn install_license_writes_verified_file() {
        let license_bytes = b"MIT License\n";
        let sha256 = sha2::Sha256::digest(license_bytes);
        let sha256_hex = hex::encode(sha256);

        let base_url = serve_once(license_bytes);
        let mut pkg = test_package(&base_url);
        pkg.license_file_sha256 = Some(sha256_hex);

        let dir = tempdir().unwrap();
        let dest = dir.path().join("LICENSE");
        fetch_verify_write(&pkg, "LICENSE", &pkg.license_file_sha256.clone(), &dest, "license")
            .unwrap();

        assert_eq!(fs::read(&dest).unwrap(), license_bytes);
    }

    #[test]
    fn install_desktop_file_writes_verified_file() {
        let desktop_bytes = b"[Desktop Entry]\nName=BreadHelp\n";
        let sha256 = sha2::Sha256::digest(desktop_bytes);
        let sha256_hex = hex::encode(sha256);

        let base_url = serve_once(desktop_bytes);
        let mut pkg = test_package(&base_url);
        pkg.desktop_file_sha256 = Some(sha256_hex);

        let dir = tempdir().unwrap();
        let dest = dir.path().join("breadhelp.desktop");
        fetch_verify_write(
            &pkg,
            "breadhelp.desktop",
            &pkg.desktop_file_sha256.clone(),
            &dest,
            "desktop entry",
        )
        .unwrap();

        assert_eq!(fs::read(&dest).unwrap(), desktop_bytes);
    }

    #[test]
    fn fetch_verify_write_refuses_checksum_mismatch() {
        let bytes = b"tampered content";
        let base_url = serve_once(bytes);
        let mut pkg = test_package(&base_url);
        pkg.license_file_sha256 = Some("0".repeat(64));

        let dir = tempdir().unwrap();
        let dest = dir.path().join("LICENSE");
        fetch_verify_write(&pkg, "LICENSE", &pkg.license_file_sha256.clone(), &dest, "license")
            .unwrap();

        // Refused, not erred (matches scaffold_config's warn-and-continue
        // posture) — the file must not have been written.
        assert!(!dest.exists());
    }

    #[test]
    fn fetch_verify_write_refuses_missing_sha256() {
        let bytes = b"some content";
        let base_url = serve_once(bytes);
        let pkg = test_package(&base_url);

        let dir = tempdir().unwrap();
        let dest = dir.path().join("LICENSE");
        fetch_verify_write(&pkg, "LICENSE", &None, &dest, "license").unwrap();

        assert!(!dest.exists());
    }

    #[test]
    fn fetch_extract_archive_extracts_tar_gz_contents() {
        // Build a real tar.gz fixture via the actual `tar` binary — matches
        // exactly what CI produces, rather than hand-rolling gzip framing.
        let src = tempdir().unwrap();
        fs::create_dir_all(src.path().join("content/tours")).unwrap();
        fs::write(
            src.path().join("content/tours/onboarding.toml"),
            b"[[step]]\n",
        )
        .unwrap();
        let archive_path = src.path().join("content.tar.gz");
        let status = Command::new("tar")
            .args(["czf"])
            .arg(&archive_path)
            .args(["-C"])
            .arg(src.path())
            .arg("content")
            .status()
            .unwrap();
        assert!(status.success());
        let archive_bytes = fs::read(&archive_path).unwrap();

        let sha256_hex = hex::encode(sha2::Sha256::digest(&archive_bytes));
        let base_url = serve_once_owned(archive_bytes);
        let pkg = test_package(&base_url);

        let dest_dir = tempdir().unwrap();
        fetch_extract_archive(&pkg, "content.tar.gz", &Some(sha256_hex), dest_dir.path())
            .unwrap();

        let extracted = dest_dir.path().join("content/tours/onboarding.toml");
        assert_eq!(fs::read(&extracted).unwrap(), b"[[step]]\n");
    }

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
