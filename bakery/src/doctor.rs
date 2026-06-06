use anyhow::Result;
use std::process::Command;

/// Check whether a list of system dependencies are present.
/// Returns (missing, warnings) — missing are hard fails, warnings are advisory.
pub fn check_deps(deps: &[String]) -> Result<Vec<String>> {
    let mut missing = Vec::new();
    for dep in deps {
        if !dep_present(dep) {
            missing.push(dep.clone());
        }
    }
    Ok(missing)
}

fn dep_present(dep: &str) -> bool {
    // Try `which` first (covers executables like `iw`, `nmcli`).
    if which(dep) {
        return true;
    }
    // Try `pkg-config --exists` for library packages (gtk4, gtk4-layer-shell, librsvg).
    pkg_config_exists(dep)
}

fn which(bin: &str) -> bool {
    Command::new("which")
        .arg(bin)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn pkg_config_exists(lib: &str) -> bool {
    // Arch package names map directly to pkg-config names for GTK libs.
    Command::new("pkg-config")
        .arg("--exists")
        .arg(lib)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Print a formatted doctor report for a list of system deps.
/// Returns true if all deps are satisfied.
pub fn report(package_name: &str, deps: &[String]) -> bool {
    if deps.is_empty() {
        println!("  {package_name}: no system deps required");
        return true;
    }
    match check_deps(deps) {
        Err(e) => {
            eprintln!("  error running doctor: {e}");
            false
        }
        Ok(missing) => {
            if missing.is_empty() {
                println!("  {package_name}: all system deps satisfied");
                true
            } else {
                eprintln!(
                    "  {package_name}: missing system deps: {}",
                    missing.join(", ")
                );
                eprintln!(
                    "  install with: sudo pacman -S {}",
                    missing.join(" ")
                );
                false
            }
        }
    }
}
