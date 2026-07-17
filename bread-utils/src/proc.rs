//! Timeout-guarded subprocess execution.
//!
//! Promoted verbatim from `breadcrumbs/src/util.rs` (the one implementation
//! in the ecosystem that already got this right — see the audit note in
//! `bread-utils`'s crate root). Several other repos shell out to
//! Wayland/Hyprland tools (`hyprctl`, `grim`, `wl-paste`, ...) via bare
//! `std::process::Command` with no timeout at all, so a hung child can wedge
//! the whole caller indefinitely. `run`/`run_with_stdin` below kill the
//! child and return a failed [`Output`] once `timeout` elapses instead.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn failed() -> Output {
        Output {
            success: false,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

/// Run a command with a hard timeout. The child is killed if it overruns so
/// a hung subprocess can never wedge the caller.
pub fn run(prog: &str, args: &[&str], timeout: Duration) -> Output {
    run_with_stdin(prog, args, None, timeout)
}

/// Like [`run`], but feeds `stdin` to the child's standard input. Useful for
/// handing secrets (e.g. Wi-Fi PSKs, API tokens) to a CLI without exposing
/// them in argv, where any local user could read them via `ps`.
pub fn run_with_stdin(prog: &str, args: &[&str], stdin: Option<&str>, timeout: Duration) -> Output {
    let stdin_cfg = if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    };
    let mut child = match Command::new(prog)
        .args(args)
        // Pin the C locale so message text callers parse (hyprctl JSON keys,
        // status output, ...) is stable regardless of the user's LANG.
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .stdin(stdin_cfg)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Output::failed(),
    };

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let out_handle = thread::spawn(move || {
        let mut buf = String::new();
        if let Some(ref mut p) = stdout_pipe {
            let _ = p.read_to_string(&mut buf);
        }
        buf
    });
    let err_handle = thread::spawn(move || {
        let mut buf = String::new();
        if let Some(ref mut p) = stderr_pipe {
            let _ = p.read_to_string(&mut buf);
        }
        buf
    });

    // Feed stdin only after the reader threads are draining stdout/stderr, so
    // a child that writes more than a pipe buffer before consuming stdin
    // can't deadlock against our blocking write.
    if let Some(data) = stdin {
        if let Some(mut sink) = child.stdin.take() {
            let _ = sink.write_all(data.as_bytes());
            // Drop closes the pipe so the child's read sees EOF.
        }
    }

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };

    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();

    Output {
        success: status.map(|s| s.success()).unwrap_or(false),
        stdout,
        stderr,
    }
}

pub fn run_ok(prog: &str, args: &[&str], timeout: Duration) -> bool {
    run(prog, args, timeout).success
}

/// Run a command and parse its stdout as JSON on success. Convenience for the
/// very common `hyprctl -j <subcommand>` / `<tool> --json` pattern.
pub fn run_json(prog: &str, args: &[&str], timeout: Duration) -> Option<serde_json::Value> {
    let out = run(prog, args, timeout);
    if !out.success {
        return None;
    }
    serde_json::from_str(&out.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_captures_stdout() {
        let out = run("printf", &["hello"], Duration::from_secs(2));
        assert!(out.success);
        assert_eq!(out.stdout, "hello");
    }

    #[test]
    fn run_reports_failure_for_nonzero_exit() {
        let out = run("sh", &["-c", "exit 3"], Duration::from_secs(2));
        assert!(!out.success);
    }

    #[test]
    fn run_kills_hung_child_after_timeout() {
        let start = Instant::now();
        let out = run("sleep", &["30"], Duration::from_millis(200));
        assert!(!out.success);
        assert!(start.elapsed() < Duration::from_secs(5), "child was not killed promptly");
    }

    #[test]
    fn run_with_stdin_feeds_child_input() {
        let out = run_with_stdin("cat", &[], Some("secret-data"), Duration::from_secs(2));
        assert!(out.success);
        assert_eq!(out.stdout, "secret-data");
    }

    #[test]
    fn run_json_parses_stdout() {
        let out = run_json("printf", &["{\"a\":1}"], Duration::from_secs(2));
        assert_eq!(out.unwrap()["a"], 1);
    }

    #[test]
    fn run_json_returns_none_on_failure() {
        let out = run_json("sh", &["-c", "exit 1"], Duration::from_secs(2));
        assert!(out.is_none());
    }
}
