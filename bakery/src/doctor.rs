use crate::ui;
use anyhow::Result;
use std::process::Command;

pub struct DepReport {
    /// Required deps that are not present — blocks install.
    pub missing: Vec<String>,
    /// Optional deps that are not present — advisory only, never blocks.
    pub warnings: Vec<String>,
}

pub fn check_deps(required: &[String], optional: &[String]) -> Result<DepReport> {
    Ok(DepReport {
        missing: required
            .iter()
            .filter(|d| !dep_present(d))
            .cloned()
            .collect(),
        warnings: optional
            .iter()
            .filter(|d| !dep_present(d))
            .cloned()
            .collect(),
    })
}

/// Arch package name -> Debian/Ubuntu package name, for the few cases where
/// they differ *and* the Debian package's own binaries don't share a name
/// with either package (so `path_has` can't bridge the gap the way it
/// already does for e.g. `ffmpeg`/`openssl`, whose package name matches
/// their own binary name on both distros). `system_deps` in `bakery.toml`
/// is always written as the Arch name — this is what makes that same
/// declaration also resolve correctly on a Debian-family bakery host like
/// hestia.
const ARCH_TO_DEBIAN_PKG: &[(&str, &str)] = &[("mkvtoolnix-cli", "mkvtoolnix")];

fn debian_name(pkg: &str) -> &str {
    ARCH_TO_DEBIAN_PKG
        .iter()
        .find(|(arch, _)| *arch == pkg)
        .map(|(_, debian)| *debian)
        .unwrap_or(pkg)
}

fn dep_present(pkg: &str) -> bool {
    // Primary: `pacman -Q` uses the exact Arch package name — no name mapping needed.
    if pacman_installed(pkg) {
        return true;
    }
    // Fallback for environments without pacman: native PATH search then pkg-config.
    if path_has(pkg) || pkg_config_exists(pkg) {
        return true;
    }
    // Further fallback for Debian/Ubuntu hosts: dpkg, via the name map above.
    dpkg_installed(debian_name(pkg))
}

fn pacman_installed(pkg: &str) -> bool {
    Command::new("pacman")
        .args(["-Q", pkg])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn dpkg_installed(pkg: &str) -> bool {
    Command::new("dpkg-query")
        .args(["-W", "-f=${Status}", pkg])
        .output()
        .map(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout).contains("install ok installed")
        })
        .unwrap_or(false)
}

/// Check PATH without shelling out to `which` (avoids the external dependency).
fn path_has(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

fn pkg_config_exists(lib: &str) -> bool {
    Command::new("pkg-config")
        .arg("--exists")
        .arg(lib)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Builds the "install with: ..." hint for a list of missing Arch package
/// names, picking the command for whichever package manager is actually on
/// this host — `sudo pacman -S ...` is meaningless advice on a Debian-family
/// bakery host like hestia, which has neither `pacman` nor the Arch names.
pub fn install_hint(missing: &[String]) -> String {
    if path_has("pacman") {
        format!("sudo pacman -S {}", missing.join(" "))
    } else if path_has("apt") {
        let names: Vec<&str> = missing.iter().map(|p| debian_name(p)).collect();
        format!("sudo apt install {}", names.join(" "))
    } else {
        format!("install: {}", missing.join(", "))
    }
}

/// Print a formatted doctor report for a package's system deps.
/// Returns true if all *required* deps are satisfied.
pub fn report(
    package_name: &str,
    required: &[String],
    optional: &[String],
    name_width: usize,
) -> bool {
    if required.is_empty() && optional.is_empty() {
        ui::check_row(true, package_name, name_width, "no system deps required");
        return true;
    }
    match check_deps(required, optional) {
        Err(e) => {
            ui::check_row(
                false,
                package_name,
                name_width,
                &format!("error running doctor: {e}"),
            );
            false
        }
        Ok(rep) => {
            for warn in &rep.warnings {
                eprintln!(
                    "  {}",
                    ui::style(
                        &format!(
                            "{package_name}: optional dep not found: {warn} \
                             (install for full functionality)"
                        ),
                        ui::YELLOW
                    )
                );
            }
            if rep.missing.is_empty() {
                ui::check_row(
                    true,
                    package_name,
                    name_width,
                    "all required system deps satisfied",
                );
                true
            } else {
                ui::check_row(
                    false,
                    package_name,
                    name_width,
                    &format!("missing: {}", rep.missing.join(", ")),
                );
                eprintln!(
                    "  {}",
                    ui::dim(&format!("install with: {}", install_hint(&rep.missing)))
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_deps_pass() {
        let rep = check_deps(&[], &[]).unwrap();
        assert!(rep.missing.is_empty());
        assert!(rep.warnings.is_empty());
    }

    // This test only runs on systems where pacman is available (Arch Linux).
    #[test]
    #[ignore]
    fn pacman_finds_itself() {
        assert!(pacman_installed("pacman"));
    }

    #[test]
    fn path_has_finds_sh() {
        assert!(path_has("sh"));
    }

    #[test]
    fn debian_name_maps_known_alias() {
        assert_eq!(debian_name("mkvtoolnix-cli"), "mkvtoolnix");
    }

    #[test]
    fn debian_name_passes_through_unmapped() {
        assert_eq!(debian_name("ffmpeg"), "ffmpeg");
    }

    // This test only runs on systems with dpkg (Debian/Ubuntu).
    #[test]
    #[ignore]
    fn dpkg_finds_dpkg_itself() {
        assert!(dpkg_installed("dpkg"));
    }

    #[test]
    fn dpkg_missing_package_not_present() {
        assert!(!dpkg_installed("this-package-does-not-exist-xyzzy42"));
    }

    #[test]
    fn missing_required_dep_detected() {
        let rep = check_deps(&["this-package-does-not-exist-xyzzy42".to_string()], &[]).unwrap();
        assert_eq!(rep.missing.len(), 1);
        assert!(rep.warnings.is_empty());
    }

    #[test]
    fn missing_optional_dep_becomes_warning_not_error() {
        let rep = check_deps(&[], &["this-package-does-not-exist-xyzzy42".to_string()]).unwrap();
        assert!(rep.missing.is_empty());
        assert_eq!(rep.warnings.len(), 1);
    }

    // This test only runs on systems where pacman is available (Arch Linux).
    #[test]
    #[ignore]
    fn installed_dep_not_missing() {
        let rep = check_deps(&["pacman".to_string()], &[]).unwrap();
        assert!(rep.missing.is_empty());
    }
}
