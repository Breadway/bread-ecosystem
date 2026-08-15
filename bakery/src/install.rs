use anyhow::{bail, Context, Result};
use std::collections::HashMap;
#[cfg(not(test))]
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::download::{fetch_and_place, verify_sha256};
use crate::manifest::{fetch_binary, Package, Service};
use crate::prefix::{self, Layout};
use crate::state::{InstalledPackage, State};
use crate::track::Track;
use crate::ui;

/// Rejects a filename that isn't a safe single path component — no `/`,
/// `\`, empty, `.`, or `..`. `bin.name`/`svc.unit`/`cfg.example`/`pkg.name`/
/// `license_file`/`desktop_file`/`data_archive` all come from the
/// minisign-verified index, so this is defense in depth (not exploitable
/// without a compromised signing key) rather than the primary guard — but
/// closing the path-traversal class is cheap enough to do anyway before any
/// of these are joined onto a fixed base directory.
fn ensure_safe_component(name: &str, what: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        bail!("refusing to install: {what} '{name}' is not a safe filename");
    }
    Ok(())
}

/// Whether stdin should be treated as an interactive terminal. Always
/// `false` in test builds regardless of the real process stdin — running
/// `cargo test` from an actual interactive shell (not CI, not piped) gives
/// the test binary a real tty, which previously made `confirm` block on a
/// `read_line` nobody was there to answer.
fn stdin_is_terminal() -> bool {
    #[cfg(test)]
    {
        false
    }
    #[cfg(not(test))]
    {
        std::io::stdin().is_terminal()
    }
}

/// Prompts `prompt [y/N] ` and returns the answer. `assume_yes` (the global
/// `--yes` flag) skips the prompt entirely; otherwise, a non-tty stdin
/// (CI, piped input) answers "no" rather than blocking on a read that will
/// never resolve.
fn confirm(prompt: &str, assume_yes: bool) -> bool {
    if assume_yes {
        return true;
    }
    if !stdin_is_terminal() {
        return false;
    }
    use std::io::Write;
    print!("{prompt} {} ", ui::dim("[y/N]"));
    std::io::stdout().flush().ok();
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf).ok();
    matches!(buf.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Installs `pkg`. `previous` is the package's current `InstalledPackage`
/// record when this is an update (looked up by the caller before starting,
/// since it's already loaded elsewhere in the call chain) — `None` for a
/// fresh first-time install. Threading it in rather than reloading `State`
/// here avoids a second load, and lets step 1 tell "update" from "fresh
/// install" for the pre-overwrite backup below.
pub fn install_package(
    pkg: &Package,
    layout: &Layout,
    track: Track,
    previous: Option<&InstalledPackage>,
    no_hooks: bool,
    assume_yes: bool,
) -> Result<()> {
    ensure_safe_component(&pkg.name, "package name")?;

    // 1. Download and verify all binaries. On an update (not a fresh
    // install), back up the current binary first — best-effort, feeding
    // `bakery rollback` — before it's overwritten below. Backups stay
    // per-user under ~/.local/state even when the live binary is in
    // /usr/local/bin, so a snapper snapshot of `@` plus this local copy
    // is enough to roll back; no second snapshot system.
    let backup_dir = previous.map(|prev| crate::state::backup_dir(&pkg.name, &prev.version));
    let mut binary_names = Vec::new();
    let mut binary_sha256 = HashMap::new();
    for bin in &pkg.binaries {
        ensure_safe_component(&bin.name, "binary name")?;
        let install_name = strip_arch_suffix(&bin.name);
        let dest = layout.bin_dir.join(install_name);
        if let Some(dir) = &backup_dir {
            backup_current_binary(dir, install_name, &dest);
        }
        let sha256 = fetch_and_place(bin, &dest)?;
        binary_names.push(install_name.to_string());
        binary_sha256.insert(install_name.to_string(), sha256);
    }

    // 2. Scaffold config dir + download example file. Config stays
    // per-user (~/.config) regardless of prefix — it's authored content,
    // not bakery-placed bits.
    if let Some(cfg) = &pkg.config {
        scaffold_config(cfg, pkg)?;
    }

    // 3. Install license file, if declared.
    if let Some(license) = &pkg.license_file {
        install_license(pkg, license, layout)?;
    }

    // 4. Install desktop entry, if declared.
    if let Some(desktop) = &pkg.desktop_file {
        install_desktop_file(pkg, desktop, layout)?;
    }

    // 5. Download + extract data archive, if declared.
    if let Some(archive) = &pkg.data_archive {
        install_data_archive(pkg, archive, layout)?;
    }

    // 6. Install systemd user units.
    let mut service_names = Vec::new();
    for svc in &pkg.services {
        install_service(svc, layout, pkg)?;
        service_names.push(svc.unit.clone());
    }

    // 7. Run post_install hooks — arbitrary `sh -c` on index-controlled
    // strings, so this is gated behind --no-hooks and an interactive
    // confirmation rather than running unconditionally.
    if !pkg.post_install.is_empty() {
        if no_hooks {
            eprintln!(
                "  {}",
                ui::note(&format!(
                    "skipped {} post_install hook(s) for {} (--no-hooks)",
                    pkg.post_install.len(),
                    pkg.name
                ))
            );
        } else if confirm(
            &format!(
                "  run {} post_install hook(s) for {}?",
                pkg.post_install.len(),
                pkg.name
            ),
            assume_yes,
        ) {
            for cmd in &pkg.post_install {
                run_hook(cmd, &pkg.name)?;
            }
        } else {
            eprintln!(
                "  {}",
                ui::note(&format!(
                    "skipped post_install hooks for {} (declined)",
                    pkg.name
                ))
            );
        }
    }

    // 8. Record in state, under an exclusive lock so a concurrent `bakery`
    // invocation can't clobber this install's record with its own.
    State::with_lock(|state| {
        state.record(InstalledPackage {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            binaries: binary_names,
            services: service_names,
            installed_at: chrono::Utc::now().to_rfc3339(),
            track,
            previous_version: previous.map(|p| p.version.clone()),
            binary_sha256,
        });
        Ok(())
    })?;

    println!("  {}", ui::ok(&format!("{} installed", pkg.name)));
    warn_path_if_needed(&layout.bin_dir);
    Ok(())
}

/// Best-effort copy of the on-disk binary into `backup_dir` before it's
/// overwritten by an update — feeds `bakery rollback`. `backup_dir` is
/// deliberately a local path (ultimately under `~/.local/state/bakery/
/// backups/<pkg>/<old-version>/`, see `state::backup_dir`) rather than
/// `bakery rollback` re-fetching the old version from `dl.breadway.dev`:
/// `index.json`'s minisign signature only covers the *current* published
/// version's checksums, so verifying an old version pulled fresh from the
/// server would only be checkable against its unsigned per-version
/// `.sha256` sidecar — a materially weaker guarantee than bakery's normal
/// trust model. A local pre-update snapshot sidesteps that gap entirely.
fn backup_current_binary(backup_dir: &Path, binary_name: &str, current_path: &Path) {
    if !current_path.exists() {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(backup_dir) {
        eprintln!(
            "  {}",
            ui::warn(&format!(
                "could not create backup dir {} ({e}) — rollback won't be available for this update",
                backup_dir.display()
            ))
        );
        return;
    }
    if let Err(e) = std::fs::copy(current_path, backup_dir.join(binary_name)) {
        eprintln!(
            "  {}",
            ui::warn(&format!(
                "could not back up {binary_name} before update ({e}) — rollback won't be available for this update"
            ))
        );
    }
}

pub fn remove_package(
    pkg_name: &str,
    layout: &Layout,
    assume_yes: bool,
    purge: bool,
) -> Result<()> {
    let installed = State::with_lock(|state| Ok(state.remove(pkg_name)))?;
    let installed = match installed {
        Some(p) => p,
        None => {
            eprintln!("  {}", ui::fail(&format!("{pkg_name} is not installed")));
            return Ok(());
        }
    };
    ui::action("Removing", pkg_name, Some(&installed.version));
    // State is already committed by with_lock above — everything from here
    // is best-effort file cleanup, and must all run even if part of it fails.

    // Remove binaries. Collect failures instead of aborting on the first one
    // so a stuck/permission-denied binary doesn't skip service removal and
    // the config/data-preserved messages below.
    let mut failures = Vec::new();
    for bin in &installed.binaries {
        let path = layout.bin_dir.join(bin);
        if path.exists() {
            match prefix::remove_file(&path) {
                Ok(()) => ui::step("removed", &path.display().to_string()),
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
    }

    // Prompt for unit removal.
    if !installed.services.is_empty() {
        let service_dir = &layout.systemd_user_dir;
        for unit in &installed.services {
            let unit_path = service_dir.join(unit);
            if confirm_remove_unit(unit, assume_yes) {
                let _ = Command::new("systemctl")
                    .args(["--user", "disable", "--now", unit])
                    .status();
                if unit_path.exists() {
                    let _ = prefix::remove_file(&unit_path);
                }
                let _ = Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .status();
                ui::step("removed", &format!("unit {unit}"));
            }
        }
    }

    // Config is never touched, even with --purge: unlike the license/desktop
    // /data paths below (all bakery-downloaded or -extracted content,
    // reproducible from source), the config dir holds user-authored/edited
    // content bakery never wrote — silently destroying it would be a bad
    // surprise no flag should cause.
    if let Some(cfg_dir) = guess_config_dir(pkg_name) {
        if cfg_dir.exists() {
            ui::step("preserved", &format!("config  {}", cfg_dir.display()));
        }
    }

    let share_dir = &layout.share_dir;
    let data_dir = share_dir.join(pkg_name);

    if purge {
        let license_dir = share_dir.join("licenses").join(pkg_name);
        remove_purged_path(&license_dir, "license dir", true, assume_yes, &mut failures);

        let desktop_file = share_dir
            .join("applications")
            .join(format!("{pkg_name}.desktop"));
        remove_purged_path(
            &desktop_file,
            "desktop entry",
            false,
            assume_yes,
            &mut failures,
        );

        remove_purged_path(&data_dir, "data dir", true, assume_yes, &mut failures);
    } else if data_dir.exists() {
        ui::step("preserved", &format!("data  {}", data_dir.display()));
    }

    if !failures.is_empty() {
        eprintln!(
            "  {}",
            ui::fail(&format!("failed to remove {} item(s):", failures.len()))
        );
        for f in &failures {
            eprintln!("    {f}");
        }
        bail!("{pkg_name} removed from state, but some files could not be deleted");
    }

    println!("  {}", ui::ok(&format!("{pkg_name} removed")));
    Ok(())
}

/// Confirms (via `confirm`, so `--yes` and non-tty stdin behave the same as
/// every other destructive prompt in this file) and removes `path` — a
/// directory when `recursive`, otherwise a single file. Declining leaves it
/// in place and prints the same "preserved at" wording the non-purge path
/// already uses. Shared by `remove_package`'s three `--purge` targets
/// (license dir, desktop entry, data dir).
fn remove_purged_path(
    path: &Path,
    label: &str,
    recursive: bool,
    assume_yes: bool,
    failures: &mut Vec<String>,
) {
    if !path.exists() {
        return;
    }
    if !confirm(
        &format!("  remove {label} at {}?", path.display()),
        assume_yes,
    ) {
        ui::step("preserved", &format!("{label}  {}", path.display()));
        return;
    }
    let result = if recursive {
        prefix::remove_dir_all(path)
    } else {
        prefix::remove_file(path)
    };
    match result {
        Ok(()) => ui::step("removed", &path.display().to_string()),
        Err(e) => failures.push(format!("{}: {e}", path.display())),
    }
}

fn scaffold_config(cfg: &crate::manifest::ConfigScaffold, pkg: &Package) -> Result<()> {
    let dir = expand_tilde(&cfg.dir);
    std::fs::create_dir_all(&dir)?;

    if let Some(example) = &cfg.example {
        ensure_safe_component(example, "config.example")?;
        let dest = dir.join(example);
        if !dest.exists() {
            if let Some((primary, fallback)) = pkg.artifact_urls(example) {
                match fetch_binary(&primary, &fallback) {
                    Ok(bytes) => match &cfg.example_sha256 {
                        Some(expected) => match verify_sha256(&bytes, expected) {
                            Ok(()) => {
                                std::fs::write(&dest, &bytes)
                                    .with_context(|| format!("writing {}", dest.display()))?;
                                ui::step("config", &dest.display().to_string());
                            }
                            Err(e) => {
                                eprintln!(
                                    "  {}",
                                    ui::warn(&format!(
                                        "checksum mismatch for example config {example}: {e} — not installed"
                                    ))
                                );
                                ui::step("config", &dir.display().to_string());
                            }
                        },
                        None => {
                            eprintln!(
                                "  {}",
                                ui::warn(&format!(
                                    "index.json has no sha256 for example config \
                                     {example} — refusing to install an unverified download"
                                ))
                            );
                            ui::step("config", &dir.display().to_string());
                        }
                    },
                    Err(e) => {
                        eprintln!(
                            "  {}",
                            ui::warn(&format!("could not download example config {example}: {e}"))
                        );
                        ui::step("config", &dir.display().to_string());
                    }
                }
            } else {
                ui::step("config", &dir.display().to_string());
            }
        } else {
            ui::step(
                "config",
                &format!("{} already exists, skipping", dest.display()),
            );
        }
    } else {
        ui::step("config", &dir.display().to_string());
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
        eprintln!(
            "  {}",
            ui::warn(&format!("no artifact URL to download {label} ({filename})"))
        );
        return Ok(());
    };
    let bytes = match fetch_binary(&primary, &fallback) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "  {}",
                ui::warn(&format!("could not download {label} {filename}: {e}"))
            );
            return Ok(());
        }
    };
    let Some(expected) = sha256 else {
        eprintln!(
            "  {}",
            ui::warn(&format!(
                "index.json has no sha256 for {label} {filename} — \
                 refusing to install an unverified download"
            ))
        );
        return Ok(());
    };
    if let Err(e) = verify_sha256(&bytes, expected) {
        eprintln!(
            "  {}",
            ui::warn(&format!(
                "checksum mismatch for {label} {filename}: {e} — not installed"
            ))
        );
        return Ok(());
    }
    prefix::write_bytes(dest, &bytes, 0o644)
        .with_context(|| format!("writing {}", dest.display()))?;
    ui::step("installed", &format!("{label}  {}", dest.display()));
    Ok(())
}

fn install_license(pkg: &Package, filename: &str, layout: &Layout) -> Result<()> {
    ensure_safe_component(filename, "license_file")?;
    let dest = layout
        .share_dir
        .join("licenses")
        .join(&pkg.name)
        .join("LICENSE");
    fetch_verify_write(pkg, filename, &pkg.license_file_sha256, &dest, "license")
}

fn install_desktop_file(pkg: &Package, filename: &str, layout: &Layout) -> Result<()> {
    ensure_safe_component(filename, "desktop_file")?;
    let dest = layout
        .share_dir
        .join("applications")
        .join(format!("{}.desktop", pkg.name));
    fetch_verify_write(
        pkg,
        filename,
        &pkg.desktop_file_sha256,
        &dest,
        "desktop entry",
    )
}

fn install_data_archive(pkg: &Package, filename: &str, layout: &Layout) -> Result<()> {
    ensure_safe_component(filename, "data_archive")?;
    let data_dir = layout.share_dir.join(&pkg.name);
    fetch_extract_archive(pkg, filename, &pkg.data_archive_sha256, &data_dir)
}

/// Downloads + verifies a `.tar.gz` artifact, then extracts it into
/// `dest_dir`. Shells out to `tar` rather than adding an archive-extraction
/// crate dependency — `tar` is universally present on Linux and this file
/// already shells out to `systemctl` for the same "trust the base system
/// has this" reason. Split from `install_data_archive` (which just supplies
/// the real `$prefix/share/<name>` destination) so tests can extract into
/// a tempdir instead.
fn fetch_extract_archive(
    pkg: &Package,
    filename: &str,
    sha256: &Option<String>,
    dest_dir: &Path,
) -> Result<()> {
    // A securely-named, process-unique temp file — the old
    // `std::env::temp_dir().join(format!("bakery-{name}-{filename}"))` was a
    // predictable path on a shared /tmp, so another local user could
    // pre-plant a symlink there for `fetch_verify_write`'s write to follow.
    let tmp_archive = tempfile::Builder::new()
        .prefix(&format!("bakery-{}-", pkg.name))
        .tempfile()
        .context("creating temp file for archive download")?
        .into_temp_path();

    fetch_verify_write(pkg, filename, sha256, &tmp_archive, "data archive")?;
    if !tmp_archive.exists() {
        // fetch_verify_write already warned (download/checksum failure).
        return Ok(());
    }

    verify_archive_paths(&tmp_archive)?;

    match prefix::extract_tar_gz(&tmp_archive, dest_dir) {
        Ok(()) => ui::step(
            "extracted",
            &format!("{filename}  →  {}", dest_dir.display()),
        ),
        Err(e) => {
            eprintln!(
                "  {}",
                ui::warn(&format!("could not extract {filename}: {e}"))
            );
        }
    }
    // `tmp_archive` (a `TempPath` guard) deletes the file when it drops here.
    Ok(())
}

/// Lists `archive_path`'s contents via `tar tvf` and rejects the archive
/// outright (no extraction) if any entry is a symlink or has an unsafe path
/// (`..` component, or absolute). `--no-same-owner --no-same-permissions` on
/// the actual extraction covers ownership/permission escalation, but not a
/// symlink or `../` entry walking the extraction outside `dest_dir` — this
/// closes that gap before `tar` ever touches disk.
fn verify_archive_paths(archive_path: &Path) -> Result<()> {
    let output = Command::new("tar")
        .arg("tvf")
        .arg(archive_path)
        .output()
        .context("listing archive contents")?;
    if !output.status.success() {
        bail!(
            "tar tvf exited with {} listing archive contents — refusing to extract",
            output.status
        );
    }

    let listing = String::from_utf8_lossy(&output.stdout);
    for line in listing.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // tar -tvf: `<perms> <owner>/<group> <size> <date> <time> <path>`,
        // with symlinks rendered as `<path> -> <target>`.
        let perms = line.split_whitespace().next().unwrap_or("");
        let is_symlink = perms.starts_with('l');

        let mut rest = line;
        for _ in 0..5 {
            let trimmed = rest.trim_start();
            let idx = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
            rest = &trimmed[idx..];
        }
        let path_field = rest.trim_start();
        let path = path_field.split(" -> ").next().unwrap_or(path_field).trim();

        if is_symlink {
            bail!("refusing to extract archive: entry '{path}' is a symlink");
        }
        if path.starts_with('/') || path.split('/').any(|c| c == "..") {
            bail!("refusing to extract archive: entry '{path}' has an unsafe path");
        }
    }
    Ok(())
}

/// Downloads and checksum-verifies `svc.unit`'s artifact.
fn fetch_and_verify_unit(pkg: &Package, svc: &Service) -> Result<Vec<u8>> {
    let (primary, fallback) = pkg
        .artifact_urls(&svc.unit)
        .ok_or_else(|| anyhow::anyhow!("no artifact URL to download {}", svc.unit))?;
    let bytes = fetch_binary(&primary, &fallback)?;
    verify_sha256(&bytes, &svc.sha256)?;
    Ok(bytes)
}

fn install_service(svc: &Service, layout: &Layout, pkg: &Package) -> Result<()> {
    ensure_safe_component(&svc.unit, "service unit")?;

    let service_dir = &layout.systemd_user_dir;
    prefix::create_dir_all(service_dir)?;

    let unit_path = service_dir.join(&svc.unit);
    let had_existing = unit_path.exists();

    // Always re-fetch and overwrite — the old `if !unit_path.exists()` gate
    // meant `Environment=`/`Restart=`/etc changes in a new release never
    // applied after the first install, unlike binaries (which always
    // re-fetch via `fetch_and_place` on every install/update). If the fetch
    // or checksum fails, fall back to whatever's already on disk rather than
    // regressing offline/flaky-network reliability. Patch ExecStart in
    // memory before the write so a system-prefix install only needs one
    // privileged write, not write-then-rewrite.
    match fetch_and_verify_unit(pkg, svc) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let patched = patch_exec_start_text(&text, &layout.bin_dir);
            prefix::write_bytes(&unit_path, patched.as_bytes(), 0o644)
                .with_context(|| format!("writing {}", unit_path.display()))?;
            ui::step("unit", &unit_path.display().to_string());
        }
        Err(e) => {
            if had_existing {
                eprintln!(
                    "  {}",
                    ui::warn(&format!(
                        "could not refresh unit {} ({e}) — keeping existing copy",
                        svc.unit
                    ))
                );
                patch_exec_start(&unit_path, &layout.bin_dir)?;
            } else {
                eprintln!(
                    "  {}",
                    ui::warn(&format!(
                        "unit file {} not found ({e}) — skipping service setup",
                        svc.unit
                    ))
                );
                return Ok(());
            }
        }
    }

    if !Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        eprintln!("  {}", ui::warn("systemctl daemon-reload failed"));
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
                ui::step("restarted", &svc.unit);
            } else {
                eprintln!("  {}", ui::warn(&format!("failed to restart {}", svc.unit)));
            }
        } else if Command::new("systemctl")
            .args(["--user", "enable", "--now", &svc.unit])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            ui::step("enabled", &svc.unit);
        } else {
            eprintln!("  {}", ui::warn(&format!("failed to enable {}", svc.unit)));
        }
    }

    Ok(())
}

fn patch_exec_start(unit_path: &Path, bin_dir: &Path) -> Result<()> {
    let text = std::fs::read_to_string(unit_path)?;
    let output = patch_exec_start_text(&text, bin_dir);
    prefix::write_bytes(unit_path, output.as_bytes(), 0o644)?;
    Ok(())
}

fn patch_exec_start_text(text: &str, bin_dir: &Path) -> String {
    let patched: String = text
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("ExecStart=") {
                let rest = line.split_once('=').map(|(_, v)| v).unwrap_or("");
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
    if text.ends_with('\n') {
        format!("{patched}\n")
    } else {
        patched
    }
}

fn run_hook(cmd: &str, pkg_name: &str) -> Result<()> {
    ui::step("hook", cmd);
    let status = Command::new("sh")
        .args(["-c", cmd])
        .status()
        .with_context(|| format!("running post_install hook for {pkg_name}"))?;
    if !status.success() {
        eprintln!("  {}", ui::warn(&format!("hook exited with {status}")));
    }
    Ok(())
}

fn confirm_remove_unit(unit: &str, assume_yes: bool) -> bool {
    confirm(&format!("  remove systemd unit {unit}?"), assume_yes)
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
        eprintln!();
        eprintln!(
            "  {}",
            ui::note(&format!(
                "{bin_str} is not in PATH — add to your shell profile:"
            ))
        );
        println!("    export PATH=\"{bin_str}:$PATH\"");
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
                let response = format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
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
                let response = format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
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
        fetch_verify_write(
            &pkg,
            "LICENSE",
            &pkg.license_file_sha256.clone(),
            &dest,
            "license",
        )
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
        fetch_verify_write(
            &pkg,
            "LICENSE",
            &pkg.license_file_sha256.clone(),
            &dest,
            "license",
        )
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
        fetch_extract_archive(&pkg, "content.tar.gz", &Some(sha256_hex), dest_dir.path()).unwrap();

        let extracted = dest_dir.path().join("content/tours/onboarding.toml");
        assert_eq!(fs::read(&extracted).unwrap(), b"[[step]]\n");
    }

    #[test]
    fn fetch_extract_archive_rejects_symlink_entries() {
        let src = tempdir().unwrap();
        std::os::unix::fs::symlink("/etc/passwd", src.path().join("evil")).unwrap();
        let archive_path = src.path().join("evil.tar.gz");
        let status = Command::new("tar")
            .args(["czf"])
            .arg(&archive_path)
            .args(["-C"])
            .arg(src.path())
            .arg("evil")
            .status()
            .unwrap();
        assert!(status.success());
        let archive_bytes = fs::read(&archive_path).unwrap();

        let sha256_hex = hex::encode(sha2::Sha256::digest(&archive_bytes));
        let base_url = serve_once_owned(archive_bytes);
        let pkg = test_package(&base_url);

        let dest_dir = tempdir().unwrap();
        let err = fetch_extract_archive(&pkg, "evil.tar.gz", &Some(sha256_hex), dest_dir.path())
            .unwrap_err();
        assert!(err.to_string().contains("symlink"));
        assert!(!dest_dir.path().join("evil").exists());
    }

    #[test]
    fn fetch_extract_archive_rejects_parent_traversal_entries() {
        // tar happily stores a `../`-prefixed member name if asked with -P
        // (disable the security checks that would otherwise strip it) —
        // this is the shape of archive our own pre-extraction check has to
        // catch, since `--no-same-owner`/`--no-same-permissions` alone don't.
        let src = tempdir().unwrap();
        fs::create_dir_all(src.path().join("payload")).unwrap();
        fs::write(src.path().join("payload/../escape.txt"), b"pwned").unwrap();
        let archive_path = src.path().join("evil.tar.gz");
        let status = Command::new("tar")
            .args(["czf"])
            .arg(&archive_path)
            .args(["-C"])
            .arg(src.path())
            .arg("-P")
            .arg("payload/../escape.txt")
            .status()
            .unwrap();
        assert!(status.success());
        let archive_bytes = fs::read(&archive_path).unwrap();

        let sha256_hex = hex::encode(sha2::Sha256::digest(&archive_bytes));
        let base_url = serve_once_owned(archive_bytes);
        let pkg = test_package(&base_url);

        let dest_dir = tempdir().unwrap();
        let err = fetch_extract_archive(&pkg, "evil.tar.gz", &Some(sha256_hex), dest_dir.path())
            .unwrap_err();
        assert!(err.to_string().contains("unsafe path"));
    }

    #[test]
    fn ensure_safe_component_accepts_plain_names() {
        assert!(ensure_safe_component("breadhelp", "x").is_ok());
        assert!(ensure_safe_component("LICENSE", "x").is_ok());
    }

    #[test]
    fn ensure_safe_component_rejects_traversal_and_separators() {
        assert!(ensure_safe_component("", "x").is_err());
        assert!(ensure_safe_component(".", "x").is_err());
        assert!(ensure_safe_component("..", "x").is_err());
        assert!(ensure_safe_component("a/b", "x").is_err());
        assert!(ensure_safe_component("a\\b", "x").is_err());
        assert!(ensure_safe_component("../../etc/passwd", "x").is_err());
    }

    #[test]
    fn install_package_rejects_unsafe_package_name() {
        let base_url = serve_once(b"unused");
        let mut pkg = test_package(&base_url);
        pkg.name = "../evil".to_string();
        let dir = tempdir().unwrap();
        let layout = Layout::from_prefix(dir.path(), None);
        let err = install_package(&pkg, &layout, Track::Stable, None, true, true).unwrap_err();
        assert!(err.to_string().contains("not a safe filename"));
    }

    #[test]
    fn confirm_returns_true_when_assume_yes() {
        assert!(confirm("anything", true));
    }

    #[test]
    fn confirm_returns_false_when_not_assume_yes_and_stdin_not_a_tty() {
        // The test harness's stdin is never an interactive terminal, so this
        // must answer "no" rather than block on a read that never resolves
        // (the CI-hang bug `confirm` replaces `confirm_remove_unit`'s old
        // unconditional stdin read to fix).
        assert!(!confirm("anything", false));
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

    #[test]
    fn backup_current_binary_copies_existing_file() {
        let dir = tempdir().unwrap();
        let current = dir.path().join("mypkg");
        fs::write(&current, b"old version bytes").unwrap();
        let backup_dir = dir.path().join("backups/mypkg/1.0.0");

        backup_current_binary(&backup_dir, "mypkg", &current);

        assert_eq!(
            fs::read(backup_dir.join("mypkg")).unwrap(),
            b"old version bytes"
        );
    }

    #[test]
    fn backup_current_binary_skips_missing_source_without_erroring() {
        // The "fresh install, nothing to back up yet" case — must not create
        // an empty backup dir or panic.
        let dir = tempdir().unwrap();
        let current = dir.path().join("does-not-exist");
        let backup_dir = dir.path().join("backups/mypkg/1.0.0");

        backup_current_binary(&backup_dir, "mypkg", &current);

        assert!(!backup_dir.exists());
    }

    #[test]
    fn remove_purged_path_removes_directory_when_confirmed() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("licenses/mypkg");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("LICENSE"), b"MIT").unwrap();
        let mut failures = Vec::new();

        remove_purged_path(&target, "license dir", true, true, &mut failures);

        assert!(!target.exists());
        assert!(failures.is_empty());
    }

    #[test]
    fn remove_purged_path_preserves_when_declined() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("mypkg.desktop");
        fs::write(&target, b"[Desktop Entry]").unwrap();
        let mut failures = Vec::new();

        // assume_yes=false and a non-tty stdin (the test harness) means
        // `confirm` answers "no" — same as `confirm_returns_false_when_not_
        // assume_yes_and_stdin_not_a_tty` above.
        remove_purged_path(&target, "desktop entry", false, false, &mut failures);

        assert!(target.exists());
        assert!(failures.is_empty());
    }

    #[test]
    fn remove_purged_path_missing_target_is_a_no_op() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("nope");
        let mut failures = Vec::new();

        remove_purged_path(&target, "data dir", true, true, &mut failures);

        assert!(failures.is_empty());
    }
}
