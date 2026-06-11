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
        missing: required.iter().filter(|d| !dep_present(d)).cloned().collect(),
        warnings: optional.iter().filter(|d| !dep_present(d)).cloned().collect(),
    })
}

fn dep_present(pkg: &str) -> bool {
    // Primary: `pacman -Q` uses the exact Arch package name — no name mapping needed.
    if pacman_installed(pkg) {
        return true;
    }
    // Fallback for environments without pacman: native PATH search then pkg-config.
    path_has(pkg) || pkg_config_exists(pkg)
}

fn pacman_installed(pkg: &str) -> bool {
    Command::new("pacman")
        .args(["-Q", pkg])
        .output()
        .map(|o| o.status.success())
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

/// Print a formatted doctor report for a package's system deps.
/// Returns true if all *required* deps are satisfied.
pub fn report(package_name: &str, required: &[String], optional: &[String]) -> bool {
    if required.is_empty() && optional.is_empty() {
        println!("  {package_name}: no system deps required");
        return true;
    }
    match check_deps(required, optional) {
        Err(e) => {
            eprintln!("  error running doctor for {package_name}: {e}");
            false
        }
        Ok(rep) => {
            for warn in &rep.warnings {
                eprintln!(
                    "  {package_name}: optional dep not found: {warn} \
                     (install for full functionality)"
                );
            }
            if rep.missing.is_empty() {
                println!("  {package_name}: all required system deps satisfied");
                true
            } else {
                eprintln!(
                    "  {package_name}: missing system deps: {}",
                    rep.missing.join(", ")
                );
                eprintln!("  install with: sudo pacman -S {}", rep.missing.join(" "));
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
    fn missing_required_dep_detected() {
        let rep = check_deps(
            &["this-package-does-not-exist-xyzzy42".to_string()],
            &[],
        )
        .unwrap();
        assert_eq!(rep.missing.len(), 1);
        assert!(rep.warnings.is_empty());
    }

    #[test]
    fn missing_optional_dep_becomes_warning_not_error() {
        let rep = check_deps(
            &[],
            &["this-package-does-not-exist-xyzzy42".to_string()],
        )
        .unwrap();
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
