//! Real panics and real segfaults, reported to a fake breadd.
//!
//! The test re-runs its own binary as a child with `BUG_FIXTURE` set; the
//! child installs reporting, prints where the failing function lives, and
//! then dies. The parent plays breadd's socket and checks what arrived.
#![cfg(all(feature = "bread-client", target_os = "linux", target_env = "gnu"))]

use std::io::{BufRead, BufReader, Read};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bread_utils::bread_client::BreadClient;
use bread_utils::bug::BugOptions;
use serde_json::Value;

#[inline(never)]
#[no_mangle]
pub fn fixture_panic_site(n: u32) -> u32 {
    if std::hint::black_box(n) > 0 {
        panic!("fixture panic {n}");
    }
    n
}

#[inline(never)]
#[no_mangle]
pub extern "C" fn fixture_segv_site(p: *mut u8) {
    // A raw store, not `ptr::write_volatile` or `*p = 1`: debug builds check
    // null and alignment before those and panic instead of faulting. The
    // store is in this function's own body, so the faulting instruction sits
    // inside the fixture's entry window.
    //
    // SAFETY: deliberately invalid; this child process is meant to crash.
    unsafe {
        std::arch::asm!("mov byte ptr [{0}], 1", in(reg) std::hint::black_box(p), options(nostack));
    }
}

fn module_offset_of(f: *const ()) -> (String, u64) {
    let ip = f as usize as u64;
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
    let exe = std::env::current_exe()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let base = maps
        .lines()
        .filter(|l| l.ends_with(&exe))
        .map(|l| u64::from_str_radix(l.split('-').next().unwrap(), 16).unwrap())
        .min()
        .unwrap();
    (exe, ip - base)
}

fn child_main(mode: &str) {
    if mode == "segv_default" {
        // Make the prior SIGSEGV action the kernel default, so a chained
        // default termination is observable as SIGSEGV exactly.
        // SAFETY: a zeroed sigaction with SIG_DFL is valid.
        unsafe {
            let mut dfl: libc::sigaction = std::mem::zeroed();
            dfl.sa_sigaction = libc::SIG_DFL;
            libc::sigaction(libc::SIGSEGV, &dfl, std::ptr::null_mut());
        }
    }
    let client = BreadClient::connect_without_bug_reporting("clip");
    {
        // SAFETY: a zeroed sigaction is a valid out-parameter.
        let mut cur: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: a null act only reads the current action.
        unsafe { libc::sigaction(libc::SIGSEGV, std::ptr::null(), &mut cur) };
        let kind = if cur.sa_sigaction == libc::SIG_DFL {
            "DFL"
        } else if cur.sa_sigaction == libc::SIG_IGN {
            "IGN"
        } else {
            "FN"
        };
        println!("PRIOR_SEGV {kind}");
    }
    let mut o = BugOptions::new("9.9.9");
    o.panic_message = std::env::var("BUG_KEEP_MESSAGE").is_ok();
    o.heartbeat = false;
    o.forward_logs = false;
    let _guard = client.enable_bug_reporting(o);
    let (path, off) = module_offset_of(if mode == "panic" {
        fixture_panic_site as *const ()
    } else {
        fixture_segv_site as *const ()
    });
    println!("SITE {path} {off}");
    if mode == "panic" {
        fixture_panic_site(7);
    } else {
        fixture_segv_site(std::ptr::null_mut());
    }
}

/// Setup-failure check: the handler cannot be installed because the socket
/// path is unusable, and nothing about the process's signals may change.
fn child_setup_failure() {
    let before: Vec<libc::sigaction> = [
        libc::SIGSEGV,
        libc::SIGBUS,
        libc::SIGILL,
        libc::SIGFPE,
        libc::SIGABRT,
    ]
    .iter()
    .map(|s| {
        // SAFETY: a zeroed sigaction is a valid out-parameter.
        let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: a null act only reads the current action.
        unsafe { libc::sigaction(*s, std::ptr::null(), &mut old) };
        old
    })
    .collect();
    let long = std::path::PathBuf::from(format!("/{}", "x".repeat(300)));
    let ok = bread_utils::bug::native::install("clip", None, &long);
    let after_sigsegv_handler = {
        // SAFETY: a zeroed sigaction is a valid out-parameter.
        let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
        // SAFETY: a null act only reads the current action.
        unsafe { libc::sigaction(libc::SIGSEGV, std::ptr::null(), &mut old) };
        old.sa_sigaction
    };
    let unchanged = before[0].sa_sigaction == after_sigsegv_handler;
    println!(
        "SETUP ok={ok} unchanged={unchanged} active={}",
        bread_utils::bug::native::is_active()
    );
}

struct Outcome {
    requests: Vec<Value>,
    site: (String, u64),
    status: std::process::ExitStatus,
}

fn run_child(mode: &str) -> Outcome {
    run_child_with(mode, true)
}

fn run_child_with(mode: &str, keep_message: bool) -> Outcome {
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("rt");
    std::fs::create_dir_all(runtime.join("bread")).unwrap();
    let listener = UnixListener::bind(runtime.join("bread/breadd.sock")).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let s3 = s2.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        s3.lock().unwrap().push(v);
                    }
                }
            });
        }
    });

    let mut cmd = Command::new(std::env::current_exe().unwrap());
    if keep_message {
        cmd.env("BUG_KEEP_MESSAGE", "1");
    }
    let mut child = cmd
        .args(["--exact", "child_entry", "--nocapture", "--test-threads=1"])
        .env("BUG_FIXTURE", mode)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("RUST_BACKTRACE", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    let status = child.wait().unwrap();
    for _ in 0..100 {
        if !seen.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    std::thread::sleep(Duration::from_millis(100));
    let site = out
        .find("SITE ")
        .map(|i| out[i + 5..].lines().next().unwrap_or(""))
        .map(|rest| {
            let (p, o) = rest.rsplit_once(' ').unwrap();
            (p.to_string(), o.trim().parse().unwrap())
        })
        .unwrap_or_else(|| panic!("child printed no SITE line; output:\n{out}"));
    let requests = seen.lock().unwrap().clone();
    Outcome {
        requests,
        site,
        status,
    }
}

/// Runs only inside the child.
#[test]
fn child_entry() {
    match std::env::var("BUG_FIXTURE").as_deref() {
        Ok("setup_failure") => child_setup_failure(),
        Ok("prior_handler") => child_prior_handler(),
        Ok(mode) => child_main(mode),
        Err(_) => {}
    }
}

#[test]
fn segv_with_default_prior_action_dies_of_sigsegv() {
    let o = run_child("segv_default");
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(
        o.status.signal(),
        Some(libc::SIGSEGV),
        "default action must terminate with SIGSEGV: {:?}",
        o.status
    );
    assert_eq!(marker_of(&o)["marker"]["signal"], "SIGSEGV");
}

/// A prior SIGSEGV handler that is not Rust std's: it writes a marker byte to
/// a pipe and exits with code 77. It stands in for a C library's handler,
/// which must keep being called after our report.
fn child_prior_handler() {
    extern "C" fn prior(_sig: libc::c_int, _info: *mut libc::siginfo_t, _ctx: *mut libc::c_void) {
        // SAFETY: write and _exit are async-signal-safe; FD 3 is the pipe the
        // parent set up for us.
        unsafe {
            let byte = b'P';
            libc::write(3, &byte as *const u8 as *const libc::c_void, 1);
            libc::_exit(77);
        }
    }
    // SAFETY: a zeroed sigaction with SA_SIGINFO and our handler is valid.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = prior as *const () as usize;
        sa.sa_flags = libc::SA_SIGINFO;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGSEGV, &sa, std::ptr::null_mut());
    }
    let client = BreadClient::connect_without_bug_reporting("clip");
    let mut o = BugOptions::new("9.9.9");
    o.panic_hook = false;
    o.heartbeat = false;
    o.forward_logs = false;
    let _guard = client.enable_bug_reporting(o);
    fixture_segv_site(std::ptr::null_mut());
}

#[test]
fn a_custom_prior_sigsegv_handler_is_still_called_after_our_report() {
    // The prior handler reports through FD 3. The parent passes the write end.
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: pipe writes two fds on success.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let (r, w) = (fds[0], fds[1]);
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("rt");
    std::fs::create_dir_all(runtime.join("bread")).unwrap();
    let listener = UnixListener::bind(runtime.join("bread/breadd.sock")).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let s3 = s2.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        s3.lock().unwrap().push(v);
                    }
                }
            });
        }
    });
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    // SAFETY: the child inherits FD 3 as the write end; nothing else is moved.
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(move || {
            if libc::dup2(w, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let status = cmd
        .args(["--exact", "child_entry", "--nocapture", "--test-threads=1"])
        .env("BUG_FIXTURE", "prior_handler")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("RUST_BACKTRACE", "0")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    // SAFETY: closing the descriptors this test opened.
    unsafe {
        libc::close(w);
    }
    let mut buf = [0u8; 1];
    // SAFETY: reading one byte from the pipe we own.
    let got = unsafe { libc::read(r, buf.as_mut_ptr() as *mut libc::c_void, 1) };
    unsafe {
        libc::close(r);
    }
    assert_eq!(
        status.code(),
        Some(77),
        "the custom prior handler must run and exit: {status:?}"
    );
    assert_eq!(got, 1, "prior handler did not write its marker");
    assert_eq!(buf[0], b'P');
    for _ in 0..100 {
        if !seen.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let reported = seen.lock().unwrap().clone();
    assert!(
        reported.iter().any(|v| v["method"] == "bug.marker"),
        "our crash marker must still be sent before chaining: {reported:?}"
    );
}

#[test]
fn a_failed_setup_leaves_every_signal_action_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_entry", "--nocapture", "--test-threads=1"])
        .env("BUG_FIXTURE", "setup_failure")
        .env("XDG_RUNTIME_DIR", dir.path())
        .env("RUST_BACKTRACE", "0")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("SETUP ok=false unchanged=true active=false"),
        "setup failure must return false and change nothing:\n{text}"
    );
}

fn marker_of(o: &Outcome) -> Value {
    o.requests
        .iter()
        .find(|r| r["method"] == "bug.marker")
        .unwrap_or_else(|| panic!("no bug.marker arrived: {:?}", o.requests))["params"]
        .clone()
}

/// The site's module as the client reports it: home directories redacted, as
/// every outgoing path is.
fn redacted(path: &str) -> String {
    bread_utils::bug::scrub::Scrubber::from_env().redact(path)
}

/// Bytes after `fixture_segv_site`'s entry that belong to the function. The
/// function is 0x28 bytes (`nm -S`); 0x80 leaves room for alignment padding
/// and a prologue change without loosening the check to a whole module.
const FIXTURE_SPAN: u64 = 0x80;

fn frames_hit_site(m: &Value, site: &(String, u64)) -> bool {
    let module = redacted(&site.0);
    m["marker"]["frames"].as_array().unwrap().iter().any(|f| {
        f["module"] == module.as_str()
            && f["offset"]
                .as_u64()
                .is_some_and(|o| o >= site.1 && o < site.1 + 512)
    })
}

#[test]
fn a_rust_panic_is_reported_with_message_location_and_a_stack_through_the_panicking_function() {
    let o = run_child("panic");
    assert_eq!(
        o.status.code(),
        Some(101),
        "the process still dies of the panic"
    );
    let p = marker_of(&o);
    assert_eq!(p["app"], "clip");
    let m = &p["marker"];
    assert_eq!(m["kind"], "panic");
    assert_eq!(m["message"], "fixture panic 7");
    assert!(m["location"].as_str().unwrap().contains("bug_crashes.rs"));
    assert_eq!(m["app_version"], "9.9.9");
    assert!(m["pid"].as_u64().unwrap() > 0);
    assert!(
        frames_hit_site(&p, &o.site),
        "stack does not pass through the panicking function (site {:?}):\n{m:#}",
        o.site
    );
    // the hook's own frames were cut: the first frame is at or after the panic site
    let first = m["frames"][0]["offset"].as_u64().unwrap();
    assert!(
        first + 4096 > o.site.1,
        "hook frames leaked into the stack top: {first:#x} vs {:#x}",
        o.site.1
    );
}

#[test]
fn a_real_segfault_is_reported_then_the_process_still_dies_of_it() {
    let o = run_child("segv");
    use std::os::unix::process::ExitStatusExt;
    // The test binary has Rust's own SIGSEGV handler installed (stack-overflow
    // detection), but ours recognizes it at install time (address in this
    // binary plus SA_SIGINFO|SA_ONSTACK) and falls back to the default action,
    // so the process still dies of the faulting signal itself. Death by
    // SIGSEGV with no prior handler at all is pinned separately in
    // `segv_with_default_prior_action_dies_of_sigsegv`.
    assert_eq!(
        o.status.signal(),
        Some(libc::SIGSEGV),
        "the process must still die of SIGSEGV: {:?}",
        o.status
    );
    let p = marker_of(&o);
    let m = &p["marker"];
    assert_eq!(m["kind"], "crash");
    assert_eq!(m["signal"], "SIGSEGV");
    assert_eq!(m["app_version"], "9.9.9");
    // The first frame is the faulting instruction, taken from the signal's
    // ucontext, so it must lie inside the fixture's own code.
    let first = &m["frames"][0];
    assert_eq!(first["module"], redacted(&o.site.0).as_str(), "{m:#}");
    let off = first["offset"].as_u64().unwrap();
    assert!(
        off >= o.site.1 && off < o.site.1 + FIXTURE_SPAN,
        "fault frame {off:#x} is outside fixture_segv_site {:#x}+{FIXTURE_SPAN:#x}:\n{m:#}",
        o.site.1
    );
    for f in m["frames"].as_array().unwrap() {
        let keys: Vec<_> = f.as_object().unwrap().keys().cloned().collect();
        assert!(
            keys.iter()
                .all(|k| ["module", "offset", "build_id"].contains(&k.as_str())),
            "{keys:?}"
        );
    }
}

#[test]
fn an_app_can_withhold_the_panic_message_but_keeps_location_and_stack() {
    let o = run_child_with("panic", false);
    let p = marker_of(&o);
    let m = &p["marker"];
    assert_eq!(m["message"], "(message omitted by the app)");
    assert!(
        !p.to_string().contains("fixture panic 7"),
        "message text leaked: {p}"
    );
    assert!(m["location"].as_str().unwrap().contains("bug_crashes.rs"));
    assert!(frames_hit_site(&p, &o.site));
}
