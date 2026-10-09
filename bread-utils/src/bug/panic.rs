//! Rust panic reporting: a panic hook that sends a `panic` marker (message,
//! location, stack as module + offset + build-id) to breadd before the
//! previous hook runs.
//!
//! Raw instruction pointers are captured with `backtrace(3)` and mapped to
//! modules via `/proc/self/maps`, so stripped release binaries still produce
//! stacks the server can symbolicate from the build-id.
//!
//! The hook must never make a bad situation worse: it is re-entrancy guarded,
//! every step is wrapped in `catch_unwind`, and the send runs on a helper
//! thread with a short deadline so a wedged breadd cannot delay process exit.

use std::cell::Cell;
use std::panic::PanicHookInfo;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};

use super::maps::Layout;
use super::send_request;

/// Most frames reported.
pub const MAX_FRAMES: usize = 64;
const SEND_DEADLINE: Duration = Duration::from_millis(400);
/// A function is never larger than this for the probe-depth search.
const PROBE_SPAN: u64 = 4096;

thread_local! {
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
    static PROBE: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Frames between the hook and the panicking function, measured once at
/// install time by panicking on purpose.
static HOOK_DEPTH: OnceLock<usize> = OnceLock::new();

/// Raw return addresses of the current stack, innermost first.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn capture_ips(max: usize) -> Vec<u64> {
    let mut buf = [std::ptr::null_mut::<libc::c_void>(); MAX_FRAMES + 32];
    let want = (max).min(buf.len()) as libc::c_int;
    // SAFETY: `buf` has at least `want` slots; backtrace writes at most that many.
    let n = unsafe { libc::backtrace(buf.as_mut_ptr(), want) };
    buf.iter()
        .take(n.max(0) as usize)
        .map(|p| *p as u64)
        .collect()
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn capture_ips(_max: usize) -> Vec<u64> {
    Vec::new()
}

#[inline(never)]
fn probe_site() {
    // Panics on purpose so the hook can measure its own depth.
    std::panic::panic_any(ProbePanic);
}

struct ProbePanic;

/// Where `ips` first enters `probe_site`, i.e. how many frames the hook and
/// panic machinery add above a panic site.
fn depth_of_probe(ips: &[u64]) -> Option<usize> {
    let start = probe_site as *const () as usize as u64;
    ips.iter()
        .position(|ip| *ip > start && *ip <= start + PROBE_SPAN)
}

fn measure_depth() -> usize {
    PROBE.with(|p| p.set(Some(0)));
    let _ = std::panic::catch_unwind(probe_site);
    let measured = PROBE.with(|p| p.take()).filter(|d| *d != 0);
    measured.map_or(0, |d| d.saturating_sub(1))
}

fn payload_message(info: &PanicHookInfo<'_>) -> String {
    let p = info.payload();
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "Box<dyn Any>".to_string())
}

/// Build the `bug.marker` params for a panic. Separate from the hook so it
/// can be tested without panicking.
pub fn marker_params(
    app: &str,
    version: Option<&str>,
    exe_build_id: Option<&str>,
    message: &str,
    location: Option<&str>,
    ips: &[u64],
    layout: &Layout,
) -> Value {
    let frames: Vec<Value> = ips
        .iter()
        .take(MAX_FRAMES)
        .filter_map(|ip| {
            let (m, off) = layout.locate(*ip)?;
            Some(json!({ "module": m.path, "offset": off, "build_id": m.build_id }))
        })
        .collect();
    json!({
        "app": app,
        "marker": {
            "kind": "panic",
            "app_version": version,
            "build_id": exe_build_id,
            "message": message.chars().take(2000).collect::<String>(),
            "location": location,
            "pid": std::process::id(),
            "frames": frames,
        }
    })
}

fn exe_build_id(layout: &Layout) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.to_str()?;
    layout
        .modules
        .iter()
        .find(|m| m.path == exe)
        .and_then(|m| m.build_id.clone())
}

/// Install the hook. The previous hook still runs afterwards.
pub fn install(app: String, version: Option<String>, socket: PathBuf) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !IN_HOOK.with(|f| f.replace(true)) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                report(&app, version.as_deref(), &socket, info);
            }));
            IN_HOOK.with(|f| f.set(false));
        }
        // The install-time probe is silent: no message from the old hook.
        if PROBE.with(Cell::get).is_none() {
            prev(info);
        }
    }));
    // Measure after the hook is in place.
    let depth = measure_depth();
    let _ = HOOK_DEPTH.set(depth);
}

fn report(app: &str, version: Option<&str>, socket: &std::path::Path, info: &PanicHookInfo<'_>) {
    let skip = HOOK_DEPTH.get().copied().unwrap_or(0);
    let mut ips = capture_ips(MAX_FRAMES + 16);
    // The install-time probe takes the same path as a real panic up to here,
    // which is what makes its measured depth valid for real panics.
    if PROBE.with(Cell::get).is_some() {
        PROBE.with(|p| p.set(depth_of_probe(&ips).map(|d| d + 1)));
        return;
    }
    ips.drain(..skip.min(ips.len()));
    let layout = Layout::current();
    let params = marker_params(
        app,
        version,
        exe_build_id(&layout).as_deref(),
        &payload_message(info),
        info.location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .as_deref(),
        &ips,
        &layout,
    );
    let socket = socket.to_path_buf();
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("bug-panic-send".into())
        .spawn(move || {
            send_request(&socket, "bug.marker", params);
            let _ = tx.send(());
        });
    if spawned.is_ok() {
        let _ = rx.recv_timeout(SEND_DEADLINE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bug::maps::{Module, Region};

    #[test]
    fn marker_params_map_ips_to_modules_and_drop_unknown_ones() {
        let layout = Layout {
            modules: vec![Module {
                path: "/usr/bin/app".into(),
                build_id: Some("aabb".into()),
                base: 0x1000,
                is_dyn: true,
            }],
            regions: vec![Region {
                start: 0x2000,
                end: 0x3000,
                module: 0,
            }],
        };
        let p = marker_params(
            "clip",
            Some("0.3"),
            Some("aabb"),
            "boom",
            Some("src/a.rs:1:2"),
            &[0x2100, 0x9999, 0x2200],
            &layout,
        );
        let m = &p["marker"];
        assert_eq!(p["app"], "clip");
        assert_eq!(m["kind"], "panic");
        assert_eq!(m["message"], "boom");
        assert_eq!(m["location"], "src/a.rs:1:2");
        let frames = m["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["offset"], 0x1100);
        assert_eq!(frames[0]["build_id"], "aabb");
    }

    #[test]
    fn long_messages_are_cut() {
        let layout = Layout::default();
        let p = marker_params("clip", None, None, &"x".repeat(10_000), None, &[], &layout);
        assert_eq!(p["marker"]["message"].as_str().unwrap().len(), 2000);
    }

    #[test]
    fn capture_returns_a_nonempty_stack_on_glibc() {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        assert!(capture_ips(16).len() >= 2);
    }

    #[test]
    fn probe_depth_search_finds_the_probe_function() {
        let start = probe_site as *const () as usize as u64;
        assert_eq!(depth_of_probe(&[1, 2, start + 10, 5]), Some(2));
        assert_eq!(depth_of_probe(&[1, 2, 3]), None);
        assert_eq!(depth_of_probe(&[start + PROBE_SPAN + 1]), None);
    }
}
