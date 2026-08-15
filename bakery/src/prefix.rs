//! Install prefix: default `~/.local`, or a system root for BOS.
//!
//! Hermes and `get.sh` keep the user-local default. BOS sets
//! `prefix = "/usr/local"` in `/etc/bakery/config.toml` (or `BAKERY_PREFIX`)
//! so bakery-managed desktop apps live on the `@` root subvolume and ride
//! along with snapper/grub-btrfs snapshots. Per-user state stays under
//! `~/.local/state/bakery` either way — bakery still records what *this*
//! user asked for; the prefix only changes where bits land on disk.
//!
//! Writes that hit `EACCES` use `sudo -n` first, then `pkexec` if a
//! graphical session is available. Interactive `sudo` (password on stdin)
//! is never used — a GUI hook must not block on a TTY prompt.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Default user-local prefix when no config/env override is set.
const DEFAULT_USER_PREFIX: &str = ".local";

/// System-wide user units, used when the prefix is not under `$HOME`.
const SYSTEM_USER_UNIT_DIR: &str = "/usr/lib/systemd/user";

const SYSTEM_CONFIG_PATH: &str = "/etc/bakery/config.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub prefix: PathBuf,
    pub bin_dir: PathBuf,
    pub share_dir: PathBuf,
    pub systemd_user_dir: PathBuf,
    /// True when `prefix` is not under the user's home directory.
    pub is_system: bool,
}

impl Layout {
    pub fn kind_label(&self) -> &'static str {
        if self.is_system {
            "system"
        } else {
            "user"
        }
    }

    /// Map a (possibly custom) prefix onto bin/share/unit paths.
    /// `bin_override` is `--bin-dir` / `BAKERY_BIN_DIR` and wins for bins only.
    pub fn from_prefix(prefix: &Path, bin_override: Option<PathBuf>) -> Self {
        let prefix = normalize_prefix_path(prefix);
        let is_system = is_system_prefix(&prefix);
        let bin_dir = bin_override.unwrap_or_else(|| prefix.join("bin"));
        let share_dir = prefix.join("share");
        let systemd_user_dir = if is_system {
            PathBuf::from(SYSTEM_USER_UNIT_DIR)
        } else {
            user_systemd_dir()
        };
        Self {
            prefix,
            bin_dir,
            share_dir,
            systemd_user_dir,
            is_system,
        }
    }

    /// Historical default: `~/.local` bins, XDG data dir for share,
    /// `~/.config/systemd/user` for units. Used when neither `BAKERY_PREFIX`
    /// nor `/etc/bakery/config.toml` sets a prefix — hermes / get.sh.
    pub fn user_default(bin_override: Option<PathBuf>) -> Self {
        let prefix = default_user_prefix();
        let bin_dir = bin_override.unwrap_or_else(|| prefix.join("bin"));
        let share_dir = dirs::data_dir().unwrap_or_else(|| prefix.join("share"));
        Self {
            prefix,
            bin_dir,
            share_dir,
            systemd_user_dir: user_systemd_dir(),
            is_system: false,
        }
    }
}

/// Resolve the active layout. `BAKERY_PREFIX` wins over `/etc/bakery/config.toml`;
/// neither set keeps the `~/.local` default. `bin_override` is the existing
/// `--bin-dir` / `BAKERY_BIN_DIR` knob.
pub fn resolve(bin_override: Option<PathBuf>) -> Layout {
    let env = std::env::var("BAKERY_PREFIX").ok();
    resolve_from(env.as_deref(), Path::new(SYSTEM_CONFIG_PATH), bin_override)
}

/// Same as [`resolve`] with the env value and config path injected, so
/// tests don't have to mutate process-global env or touch `/etc`.
pub fn resolve_from(
    env_prefix: Option<&str>,
    config_path: &Path,
    bin_override: Option<PathBuf>,
) -> Layout {
    match configured_prefix_from(env_prefix, config_path) {
        Some(prefix) => Layout::from_prefix(&prefix, bin_override),
        None => Layout::user_default(bin_override),
    }
}

pub fn configured_prefix_from(env_prefix: Option<&str>, config_path: &Path) -> Option<PathBuf> {
    if let Some(raw) = env_prefix {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Some(normalize_prefix(trimmed));
        }
    }
    load_config_prefix(config_path)
}

#[derive(Debug, Default, Deserialize)]
struct BakeryConfig {
    prefix: Option<String>,
}

/// Reads `prefix = "..."` from a bakery config file. Missing file or empty
/// key → `None` (caller falls back to the user-local default). A file that
/// exists but fails to parse is warned about, not treated as fatal — a typo
/// in `/etc/bakery/config.toml` must not take down `bakery list`.
pub fn load_config_prefix(path: &Path) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!(
                "  {}",
                crate::ui::warn(&format!("could not read {}: {e}", path.display()))
            );
            return None;
        }
    };
    match toml::from_str::<BakeryConfig>(&text) {
        Ok(cfg) => cfg
            .prefix
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(normalize_prefix),
        Err(e) => {
            eprintln!(
                "  {}",
                crate::ui::warn(&format!("could not parse {}: {e}", path.display()))
            );
            None
        }
    }
}

fn default_user_prefix() -> PathBuf {
    home_dir().join(DEFAULT_USER_PREFIX)
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"))
}

fn user_systemd_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| home_dir().join(".config"))
        .join("systemd/user")
}

fn is_system_prefix(prefix: &Path) -> bool {
    match dirs::home_dir() {
        Some(home) => !prefix.starts_with(&home),
        None => true,
    }
}

fn normalize_prefix(raw: &str) -> PathBuf {
    normalize_prefix_path(&expand_tilde(raw))
}

fn normalize_prefix_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        home_dir()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(path)
    }
}

fn is_permission_denied(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::PermissionDenied
}

pub fn privilege_denied_msg(dest: &Path) -> String {
    format!(
        "permission denied writing {} — need root for this prefix. \
         bakery tried `sudo -n` then `pkexec`; neither succeeded. \
         Run from a root shell, grant passwordless sudo -n for install/rm/tar, \
         or install a polkit rule. bakery will not prompt for a sudo password.",
        dest.display()
    )
}

fn has_graphical_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}

/// Write `bytes` to `dest`, creating parent dirs. Escalates on `EACCES`.
pub fn write_bytes(dest: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    match bread_utils::atomic::write_atomic_bytes(dest, bytes, Some(mode)) {
        Ok(()) => Ok(()),
        Err(e) if is_permission_denied(&e) => write_bytes_privileged(dest, bytes, mode),
        Err(e) => Err(e).with_context(|| format!("writing {}", dest.display())),
    }
}

fn write_bytes_privileged(dest: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let mut tmp =
        tempfile::NamedTempFile::new().context("creating temp file for privileged write")?;
    tmp.write_all(bytes)
        .and_then(|_| tmp.flush())
        .and_then(|_| tmp.as_file().sync_all())
        .context("writing temp file for privileged write")?;
    let mode_str = format!("{mode:o}");
    run_privileged(
        Path::new("/usr/bin/install"),
        &[
            OsStr::new("-D"),
            OsStr::new("-m"),
            OsStr::new(&mode_str),
            tmp.path().as_os_str(),
            dest.as_os_str(),
        ],
        dest,
    )
}

pub fn create_dir_all(path: &Path) -> Result<()> {
    match std::fs::create_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if is_permission_denied(&e) => run_privileged(
            Path::new("/usr/bin/install"),
            &[
                OsStr::new("-d"),
                OsStr::new("-m"),
                OsStr::new("755"),
                path.as_os_str(),
            ],
            path,
        ),
        Err(e) => Err(e).with_context(|| format!("creating directory {}", path.display())),
    }
}

pub fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) if is_permission_denied(&e) => run_privileged(
            Path::new("/usr/bin/rm"),
            &[OsStr::new("-f"), path.as_os_str()],
            path,
        ),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

pub fn remove_dir_all(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) if is_permission_denied(&e) => run_privileged(
            Path::new("/usr/bin/rm"),
            &[OsStr::new("-rf"), path.as_os_str()],
            path,
        ),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// Extract `archive` (a `.tar.gz`) into `dest_dir`. Escalates the `tar`
/// invocation when `dest_dir` is not writable by this user — typical for
/// `$prefix/share/<pkg>` under `/usr/local`.
pub fn extract_tar_gz(archive: &Path, dest_dir: &Path) -> Result<()> {
    create_dir_all(dest_dir)?;
    if dir_writable_by_self(dest_dir) {
        let status = Command::new("tar")
            .args([
                "xzf",
                &archive.to_string_lossy(),
                "--no-same-owner",
                "--no-same-permissions",
                "-C",
            ])
            .arg(dest_dir)
            .status()
            .with_context(|| format!("running tar to extract {}", archive.display()))?;
        if !status.success() {
            bail!("tar exited with {status} extracting {}", archive.display());
        }
        return Ok(());
    }
    run_privileged(
        Path::new("/usr/bin/tar"),
        &[
            OsStr::new("xzf"),
            archive.as_os_str(),
            OsStr::new("--no-same-owner"),
            OsStr::new("--no-same-permissions"),
            OsStr::new("-C"),
            dest_dir.as_os_str(),
        ],
        dest_dir,
    )
}

fn dir_writable_by_self(dir: &Path) -> bool {
    tempfile::Builder::new()
        .prefix(".bakery-wprobe-")
        .tempfile_in(dir)
        .is_ok()
}

fn run_privileged(program: &Path, args: &[&OsStr], dest: &Path) -> Result<()> {
    // `sudo -n` never prompts; stdin is null so a misconfigured sudoers
    // can't fall through to a password read on a GUI hook's non-tty stdin.
    let sudo = Command::new("sudo")
        .arg("-n")
        .arg(program)
        .args(args)
        .stdin(Stdio::null())
        .status();
    if matches!(sudo, Ok(status) if status.success()) {
        return Ok(());
    }

    // pkexec pops a polkit dialog — only useful with a display, and the
    // one acceptable password prompt (GUI, not a stolen sudo TTY).
    if has_graphical_session() {
        let pk = Command::new("pkexec").arg(program).args(args).status();
        if matches!(pk, Ok(status) if status.success()) {
            return Ok(());
        }
    }

    bail!("{}", privilege_denied_msg(dest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn user_default_is_not_system_and_uses_local_bin() {
        let layout = Layout::user_default(None);
        assert!(!layout.is_system);
        assert_eq!(layout.prefix, default_user_prefix());
        assert_eq!(layout.bin_dir, default_user_prefix().join("bin"));
        assert_eq!(layout.kind_label(), "user");
        assert!(layout.systemd_user_dir.ends_with(Path::new("systemd/user")));
        assert_ne!(layout.systemd_user_dir, PathBuf::from(SYSTEM_USER_UNIT_DIR));
    }

    #[test]
    fn user_default_honors_bin_override() {
        let layout = Layout::user_default(Some(PathBuf::from("/tmp/custom-bins")));
        assert_eq!(layout.bin_dir, PathBuf::from("/tmp/custom-bins"));
        assert!(!layout.is_system);
        assert_eq!(layout.prefix, default_user_prefix());
    }

    #[test]
    fn usr_local_is_system_layout() {
        let layout = Layout::from_prefix(Path::new("/usr/local"), None);
        assert!(layout.is_system);
        assert_eq!(layout.prefix, PathBuf::from("/usr/local"));
        assert_eq!(layout.bin_dir, PathBuf::from("/usr/local/bin"));
        assert_eq!(layout.share_dir, PathBuf::from("/usr/local/share"));
        assert_eq!(layout.systemd_user_dir, PathBuf::from(SYSTEM_USER_UNIT_DIR));
        assert_eq!(layout.kind_label(), "system");
    }

    #[test]
    fn custom_home_prefix_is_not_system() {
        let home = dirs::home_dir().expect("home dir");
        let prefix = home.join("apps");
        let layout = Layout::from_prefix(&prefix, None);
        assert!(!layout.is_system);
        assert_eq!(layout.bin_dir, prefix.join("bin"));
        assert_eq!(layout.share_dir, prefix.join("share"));
        assert_ne!(layout.systemd_user_dir, PathBuf::from(SYSTEM_USER_UNIT_DIR));
    }

    #[test]
    fn temp_prefix_maps_bin_and_share_under_prefix() {
        let dir = tempdir().unwrap();
        let layout = Layout::from_prefix(dir.path(), None);
        assert_eq!(layout.bin_dir, dir.path().join("bin"));
        assert_eq!(layout.share_dir, dir.path().join("share"));
        // /tmp is not under $HOME, so this is a system-shaped prefix —
        // units would go to /usr/lib/systemd/user. Writes still try
        // unprivileged first, so tests can use a temp prefix without sudo.
        assert!(layout.is_system);
        assert_eq!(layout.systemd_user_dir, PathBuf::from(SYSTEM_USER_UNIT_DIR));
    }

    #[test]
    fn bin_override_does_not_move_share_or_units() {
        let layout = Layout::from_prefix(
            Path::new("/usr/local"),
            Some(PathBuf::from("/opt/override/bin")),
        );
        assert_eq!(layout.bin_dir, PathBuf::from("/opt/override/bin"));
        assert_eq!(layout.share_dir, PathBuf::from("/usr/local/share"));
        assert_eq!(layout.systemd_user_dir, PathBuf::from(SYSTEM_USER_UNIT_DIR));
    }

    #[test]
    fn load_config_prefix_reads_value() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "prefix = \"/usr/local\"\n").unwrap();
        assert_eq!(load_config_prefix(&path), Some(PathBuf::from("/usr/local")));
    }

    #[test]
    fn load_config_prefix_expands_tilde() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "prefix = \"~/.local\"\n").unwrap();
        assert_eq!(load_config_prefix(&path), Some(default_user_prefix()));
    }

    #[test]
    fn load_config_prefix_missing_file_is_none() {
        assert_eq!(
            load_config_prefix(Path::new("/no/such/bakery-config.toml")),
            None
        );
    }

    #[test]
    fn load_config_prefix_ignores_empty_value() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "prefix = \"\"\n").unwrap();
        assert_eq!(load_config_prefix(&path), None);
    }

    #[test]
    fn load_config_prefix_malformed_is_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "prefix = [\n").unwrap();
        assert_eq!(load_config_prefix(&path), None);
    }

    #[test]
    fn env_prefix_wins_over_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "prefix = \"/usr/local\"\n").unwrap();
        let layout = resolve_from(Some("/opt/bread"), &path, None);
        assert_eq!(layout.prefix, PathBuf::from("/opt/bread"));
        assert_eq!(layout.bin_dir, PathBuf::from("/opt/bread/bin"));
        assert!(layout.is_system);
    }

    #[test]
    fn empty_env_falls_through_to_config() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "prefix = \"/usr/local\"\n").unwrap();
        let layout = resolve_from(Some("   "), &path, None);
        assert_eq!(layout.prefix, PathBuf::from("/usr/local"));
    }

    #[test]
    fn no_env_no_config_is_user_default() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.toml");
        let layout = resolve_from(None, &path, None);
        assert_eq!(layout, Layout::user_default(None));
    }

    #[test]
    fn write_bytes_to_writable_temp_prefix_needs_no_root() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("bin").join("foo");
        write_bytes(&dest, b"hello", 0o755).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
    }

    #[test]
    fn create_and_remove_under_temp_prefix() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("share/licenses/pkg");
        create_dir_all(&nested).unwrap();
        assert!(nested.is_dir());
        let file = nested.join("LICENSE");
        write_bytes(&file, b"MIT\n", 0o644).unwrap();
        remove_file(&file).unwrap();
        assert!(!file.exists());
        remove_dir_all(&dir.path().join("share")).unwrap();
        assert!(!dir.path().join("share").exists());
    }

    #[test]
    fn privilege_denied_msg_names_the_dest() {
        let msg = privilege_denied_msg(Path::new("/usr/local/bin/breadd"));
        assert!(msg.contains("/usr/local/bin/breadd"));
        assert!(msg.contains("sudo -n"));
        assert!(msg.contains("pkexec"));
        assert!(msg.contains("will not prompt"));
    }

    #[test]
    fn is_system_prefix_classifies_home_and_usr() {
        let home = dirs::home_dir().expect("home dir");
        assert!(!is_system_prefix(&home.join(".local")));
        assert!(is_system_prefix(Path::new("/usr/local")));
        assert!(is_system_prefix(Path::new("/opt/bread")));
    }
}
