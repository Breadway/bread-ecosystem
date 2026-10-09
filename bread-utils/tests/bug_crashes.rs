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
    // SAFETY: deliberately invalid; this child process is meant to crash.
    unsafe { std::ptr::write_volatile(std::hint::black_box(p), 1) };
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
    let client = BreadClient::connect("clip");
    let mut o = BugOptions::new("9.9.9");
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

struct Outcome {
    requests: Vec<Value>,
    site: (String, u64),
    status: std::process::ExitStatus,
}

fn run_child(mode: &str) -> Outcome {
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

    let mut child = Command::new(std::env::current_exe().unwrap())
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
    if let Ok(mode) = std::env::var("BUG_FIXTURE") {
        child_main(&mode);
    }
}

fn marker_of(o: &Outcome) -> Value {
    o.requests
        .iter()
        .find(|r| r["method"] == "bug.marker")
        .unwrap_or_else(|| panic!("no bug.marker arrived: {:?}", o.requests))["params"]
        .clone()
}

fn frames_hit_site(m: &Value, site: &(String, u64)) -> bool {
    m["marker"]["frames"].as_array().unwrap().iter().any(|f| {
        f["module"] == site.0.as_str()
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
        "stack does not pass through the panicking function:\n{m:#}"
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
    assert_eq!(
        o.status.signal(),
        Some(libc::SIGSEGV),
        "default action must still happen: {:?}",
        o.status
    );
    let p = marker_of(&o);
    let m = &p["marker"];
    assert_eq!(m["kind"], "crash");
    assert_eq!(m["signal"], "SIGSEGV");
    assert_eq!(m["app_version"], "9.9.9");
    assert!(
        frames_hit_site(&p, &o.site),
        "stack does not pass through the faulting function:\n{m:#}"
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
