//! Atomic file writes: write to a sibling temp file, then `rename` over the
//! target so a crash, power loss, or disk-full error mid-write never leaves
//! a truncated/corrupt file behind (a same-filesystem rename is atomic).
//!
//! Two flavors, both extracted from real (and identical) duplication:
//!
//! - [`write_atomic`] — temp-then-rename, with an optional Unix `mode` set
//!   up front (so secrets never exist world-readable even briefly). This is
//!   `breadcrumbs/src/util.rs::write_atomic`, promoted verbatim.
//! - [`write_atomic_backed_up`] — temp-then-rename *plus* a best-effort
//!   `<path>.bak` copy of whatever was there before, so a successful-but-wrong
//!   write is always recoverable. This is `bos-settings/src/config/mod.rs`'s
//!   `atomic_write`, which `breadhelp/src/config.rs` re-implemented
//!   byte-for-byte in the same fix pass that introduced it (its own doc
//!   comment says "same discipline as bos-settings/src/config/mod.rs") —
//!   exactly the kind of fresh duplication this crate exists to remove.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Write `contents` to `path` atomically. `mode` (Unix only) is applied to
/// the temp file *before* any data is written, so a file that must stay
/// private (secrets, tokens) is never briefly world-readable.
pub fn write_atomic(path: &Path, contents: &str, mode: Option<u32>) -> io::Result<()> {
    write_atomic_bytes(path, contents.as_bytes(), mode)
}

/// Byte-oriented sibling of [`write_atomic`], for binary payloads (e.g. a
/// downloaded ONNX model file — see `bread-onnx`'s downloader).
pub fn write_atomic_bytes(path: &Path, contents: &[u8], mode: Option<u32>) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = tmp_path(path, dir);

    let mut open = fs::OpenOptions::new();
    open.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;

    let res = (|| {
        use std::io::Write;
        let mut f = open.open(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// Like [`write_atomic`] (no `mode`), but first best-effort copies whatever
/// is currently at `path` to `<path>.bak`. The backup is best-effort — a
/// failure to back up (e.g. read-only source, first-ever write) does not
/// block the write itself.
pub fn write_atomic_backed_up(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let backup = backup_path(path);
        let _ = fs::copy(path, &backup);
    }
    write_atomic(path, contents, None)
}

fn tmp_path(path: &Path, dir: &Path) -> PathBuf {
    let stem = path.file_name().and_then(|s| s.to_str()).unwrap_or("bread");
    dir.join(format!(".{stem}.tmp.{}", std::process::id()))
}

fn backup_path(path: &Path) -> PathBuf {
    backup_path_for(path)
}

/// `<path>.bak` — shared with [`crate::tomlcfg`] so its own backup-before-
/// falling-back-to-defaults logging points at the same file this module
/// would have backed up to on a write.
pub(crate) fn backup_path_for(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.bak", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bread-utils-atomic-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_atomic_creates_file_with_contents() {
        let dir = tmp_dir("basic");
        let path = dir.join("config.toml");
        write_atomic(&path, "hello", None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_leaves_no_tmp_file_behind() {
        let dir = tmp_dir("no-leftover");
        let path = dir.join("config.toml");
        write_atomic(&path, "hello", None).unwrap();
        let leftover: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(leftover.is_empty(), "leftover tmp files: {leftover:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_applies_mode_before_any_data_hits_disk() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp_dir("mode");
        let path = dir.join("secret");
        write_atomic(&path, "token", Some(0o600)).unwrap();
        let perms = fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_backed_up_backs_up_previous_contents() {
        let dir = tmp_dir("backup");
        let path = dir.join("state.toml");
        let backup = dir.join("state.toml.bak");

        write_atomic_backed_up(&path, "first").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first");
        assert!(!backup.exists(), "no backup should exist before the first overwrite");

        write_atomic_backed_up(&path, "second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "first");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_backed_up_leaves_no_tmp_file_behind() {
        let dir = tmp_dir("backup-no-leftover");
        let path = dir.join("state.toml");
        write_atomic_backed_up(&path, "first").unwrap();
        write_atomic_backed_up(&path, "second").unwrap();
        let leftover: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(leftover.is_empty(), "leftover tmp files: {leftover:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_overwrite_never_leaves_partial_contents_visible() {
        // Not a true crash-injection test (hard to do portably), but pins
        // down the observable contract: after a successful call, the file
        // is either fully old or fully new, never truncated.
        let dir = tmp_dir("no-partial");
        let path = dir.join("f");
        write_atomic(&path, "aaaaaaaaaa", None).unwrap();
        write_atomic(&path, "b", None).unwrap();
        let mut s = String::new();
        fs::File::open(&path).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "b");
        let _ = fs::remove_dir_all(&dir);
    }
}
