//! Correct single-instance / PID-toggle, replacing the TOCTOU-prone pattern
//! duplicated in `breadbox/src/main.rs` (`toggle_or_continue`/`pid_file`,
//! ~30 lines) and `breadclip/src/main.rs` (same function names, whose own
//! comment reads `// ---- PID file toggle (single-instance, matches breadbox
//! pattern) ----`).
//!
//! The old pattern: read the PID file, `/proc/<pid>/comm`-check whether it's
//! still this app, `kill` it if so, otherwise `fs::write` our own PID over
//! it. That's three separate, non-atomic steps — two instances launched at
//! once can both read "no valid PID" and both proceed as the "first"
//! instance; a stale PID file left by a crash can also collide with an
//! unrelated process that was later assigned the same PID by the kernel,
//! sending it a `kill` it never asked for.
//!
//! This module instead holds an exclusive, kernel-atomic advisory lock
//! (`std::fs::File::try_lock`, i.e. `flock(2)`) on the PID file for the
//! entire lifetime of the process that acquires it. Lock ownership itself
//! *is* the liveness check — there is no window where two processes can
//! both believe they're the sole instance, and a crashed process's lock is
//! released by the kernel the instant it dies, so there's no stale-lock
//! case to reason about at all.
//!
//! [`try_acquire`] is the side-effect-free primitive (no signals sent);
//! [`toggle_or_kill`] layers breadbox/breadclip's actual desired behavior
//! (kill whoever's running, then exit) on top of it.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

/// Held for the lifetime of the running instance. Dropping it releases the
/// flock and removes the PID file. Keep this alive (e.g. in a `let _guard =
/// ...` bound in `main`) for as long as the app should be considered "the"
/// running instance.
pub struct Guard {
    _file: File,
    path: PathBuf,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub enum Acquire {
    /// No other instance was running; we now hold the lock.
    Acquired(Guard),
    /// Another instance already holds the lock and is therefore alive right
    /// now. Carries whatever PID it last recorded, if the file contents
    /// parsed as one.
    HeldByOther(Option<u32>),
}

pub enum Toggle {
    /// No other instance was running; we now hold the lock. Keep the guard
    /// alive for the process's lifetime.
    Started(Guard),
    /// Another instance was already running and (if a PID could be read
    /// from the file) has been sent `SIGTERM`. The caller should exit
    /// immediately without starting.
    KilledExisting,
}

/// `$XDG_RUNTIME_DIR/<app>.pid` (falling back to `/tmp`, matching every
/// existing consumer's own fallback) — same location `breadbox`/`breadclip`
/// already used.
pub fn pid_file_path(app: &str) -> PathBuf {
    crate::xdg::runtime_dir().join(format!("{app}.pid"))
}

/// Try to become the single instance of `app`, with no side effects beyond
/// the lock/file itself — in particular, unlike [`toggle_or_kill`], this
/// never signals another process. Prefer this if your app wants different
/// behavior than "kill the existing instance" (e.g. just refuse to start a
/// second copy).
pub fn try_acquire(app: &str) -> std::io::Result<Acquire> {
    let path = pid_file_path(app);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)?;

    match file.try_lock() {
        Ok(()) => {
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            write!(file, "{}", std::process::id())?;
            file.sync_all()?;
            Ok(Acquire::Acquired(Guard { _file: file, path }))
        }
        Err(_) => {
            let mut contents = String::new();
            let _ = file.read_to_string(&mut contents);
            Ok(Acquire::HeldByOther(contents.trim().parse::<u32>().ok()))
        }
    }
}

/// Toggle behavior: acquire the single-instance lock for `app`. If already
/// held by another live process, signal it to quit (`SIGTERM` via `kill`)
/// and return [`Toggle::KilledExisting`] — the caller should exit. Otherwise
/// take the lock and return [`Toggle::Started`] — the caller should proceed
/// and keep the guard alive.
pub fn toggle_or_kill(app: &str) -> std::io::Result<Toggle> {
    Ok(match try_acquire(app)? {
        Acquire::Acquired(guard) => Toggle::Started(guard),
        Acquire::HeldByOther(Some(pid)) => {
            kill(pid);
            Toggle::KilledExisting
        }
        Acquire::HeldByOther(None) => Toggle::KilledExisting,
    })
}

#[cfg(unix)]
fn kill(pid: u32) {
    // Shells out rather than binding libc directly, matching how every
    // existing consumer already did this (`Command::new("kill")`) — no new
    // dependency for a one-shot signal.
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status();
}

#[cfg(not(unix))]
fn kill(_pid: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn unique_app(name: &str) -> String {
        format!("bread-utils-singleton-test-{name}-{}", std::process::id())
    }

    #[test]
    fn first_acquire_succeeds_and_releases_on_drop() {
        let app = unique_app("first");
        match try_acquire(&app).unwrap() {
            Acquire::Acquired(_guard) => {}
            Acquire::HeldByOther(_) => panic!("expected to be the first instance"),
        }
        // Guard dropped at end of scope; pid file should be gone.
        std::thread::sleep(Duration::from_millis(10));
        assert!(!pid_file_path(&app).exists());
    }

    #[test]
    fn second_acquire_while_first_is_held_reports_held_by_other_with_our_pid() {
        let app = unique_app("second");
        let guard = match try_acquire(&app).unwrap() {
            Acquire::Acquired(g) => g,
            Acquire::HeldByOther(_) => panic!("expected to be the first instance"),
        };

        // A second attempt while the first guard is still held must not be
        // able to acquire the lock too — that's the whole point. No signal
        // is sent by `try_acquire` itself (that's `toggle_or_kill`'s job),
        // so this is safe to assert without affecting the test process.
        match try_acquire(&app).unwrap() {
            Acquire::HeldByOther(pid) => assert_eq!(pid, Some(std::process::id())),
            Acquire::Acquired(_) => panic!("second acquire succeeded while the first still holds the lock"),
        }

        drop(guard);
    }

    #[test]
    fn lock_is_released_after_guard_drop_so_a_later_instance_can_acquire() {
        let app = unique_app("release");
        let guard = match try_acquire(&app).unwrap() {
            Acquire::Acquired(g) => g,
            Acquire::HeldByOther(_) => panic!("expected to be the first instance"),
        };
        drop(guard);

        match try_acquire(&app).unwrap() {
            Acquire::Acquired(_g) => {}
            Acquire::HeldByOther(_) => panic!("lock should have been released when the guard was dropped"),
        }
    }

    #[test]
    fn toggle_or_kill_starts_when_nothing_else_is_running() {
        let app = unique_app("toggle-start");
        match toggle_or_kill(&app).unwrap() {
            Toggle::Started(_guard) => {}
            Toggle::KilledExisting => panic!("expected to start as the first instance"),
        }
    }

    // Deliberately not unit-tested: `toggle_or_kill`'s kill-the-existing-
    // instance branch. Exercising it for real means sending a real SIGTERM
    // to a real process; the only PID a test process can safely target is
    // its own (as a stand-in "other instance" via a shared PID file), and
    // doing that would SIGTERM the test binary itself. The branch is a
    // two-line, directly-inspectable call to `kill()` gated on
    // `HeldByOther(Some(pid))`, which the `second_acquire_...` test above
    // already exercises up to (and excluding) the signal send.
}
