//! Native crash reporting: a signal handler for SIGSEGV, SIGBUS, SIGILL,
//! SIGFPE and SIGABRT that sends a `crash` marker (signal name + stack as
//! module + offset + build-id) to breadd, then lets the process die the way
//! it would have.
//!
//! This is the in-process option from BREAD-17 (the other being
//! systemd-coredump). It was chosen because it needs no privileges or system
//! configuration and works for any user session. Frames only: no memory, no
//! registers, no environment is ever read or sent.
//!
//! # Signal safety
//!
//! Everything the handler does is prepared at install time: the request text
//! is pre-rendered in three pieces, the module table is a leaked snapshot,
//! the socket address is prebuilt, `backtrace(3)` is pre-warmed so its
//! lazy initialisation cannot allocate later. The handler itself uses a
//! fixed stack buffer, `socket`/`connect`/`write`/`close` and the faulting
//! signal's default action; it allocates nothing. It runs on an alternate
//! stack so stack overflows are reportable. The snapshot misses libraries
//! loaded after install; their frames are simply omitted.

use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use super::maps::Layout;

const SIGNALS: &[(libc::c_int, &str)] = &[
    (libc::SIGSEGV, "SIGSEGV"),
    (libc::SIGBUS, "SIGBUS"),
    (libc::SIGILL, "SIGILL"),
    (libc::SIGFPE, "SIGFPE"),
    (libc::SIGABRT, "SIGABRT"),
];
const BUF: usize = 16 * 1024;
const MAX_FRAMES: usize = 48;
/// Frames belonging to the handler and the signal trampoline.
const SKIP: usize = 2;
const ALT_STACK: usize = 128 * 1024;

struct State {
    head: Vec<u8>,
    middle: Vec<u8>,
    tail: Vec<u8>,
    addr: libc::sockaddr_un,
    addr_len: libc::socklen_t,
    layout: Layout,
}

static STATE: OnceLock<State> = OnceLock::new();
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Pre-render the request in three pieces around the signal name and the
/// frame list: `head` + `"SIGNAME"` + `middle` + `[frames]` + `tail`.
pub fn render_pieces(
    app: &str,
    version: Option<&str>,
    build_id: Option<&str>,
) -> (String, String, String) {
    let js = |v: &str| serde_json::to_string(v).unwrap_or_else(|_| "\"\"".into());
    let opt = |v: Option<&str>| v.map_or_else(|| "null".to_string(), js);
    // Written by hand (not a json! map) so the key order is exactly
    // signal-then-frames, which the three-piece split below relies on.
    (
        format!(
            "{{\"id\":\"0\",\"method\":\"bug.marker\",\"params\":{{\"app\":{},\"marker\":{{\"kind\":\"crash\",\"app_version\":{},\"build_id\":{},\"pid\":{},\"signal\":",
            js(app),
            opt(version),
            opt(build_id),
            std::process::id()
        ),
        ",\"frames\":".to_string(),
        "}}}\n".to_string(),
    )
}

/// A fixed buffer that never allocates and silently stops when full.
struct Out {
    buf: [u8; BUF],
    len: usize,
}

impl Out {
    fn new() -> Self {
        Self {
            buf: [0; BUF],
            len: 0,
        }
    }

    fn put(&mut self, b: &[u8]) {
        let n = b.len().min(BUF - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
        self.len += n;
    }

    fn put_u64(&mut self, mut v: u64) {
        let mut tmp = [0u8; 20];
        let mut i = tmp.len();
        loop {
            i -= 1;
            tmp[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.put(&tmp[i..]);
    }

    /// JSON string contents: quotes, backslashes and control bytes dropped.
    fn put_json_str(&mut self, s: &[u8]) {
        for &c in s {
            if c >= 0x20 && c != b'"' && c != b'\\' && c < 0x7f {
                self.put(&[c]);
            }
        }
    }

    fn remaining(&self) -> usize {
        BUF - self.len
    }
}

/// Build the whole request into `out`. Pure function of its inputs (no
/// syscalls), so it can be tested directly.
fn build(out: &mut Out, st: &State, signal: &str, ips: &[u64]) {
    out.put(&st.head);
    out.put(b"\"");
    out.put(signal.as_bytes());
    out.put(b"\"");
    out.put(&st.middle);
    out.put(b"[");
    let mut first = true;
    for ip in ips.iter().take(MAX_FRAMES) {
        let Some((m, off)) = st.layout.locate(*ip) else {
            continue;
        };
        // Whole frames only: leave room for this frame (worst case), the
        // separator, the closing bracket and the tail, or stop here.
        let need =
            m.path.len() + m.build_id.as_ref().map_or(0, String::len) + 80 + 1 + 1 + st.tail.len();
        if out.remaining() < need {
            break;
        }
        if !first {
            out.put(b",");
        }
        first = false;
        out.put(b"{\"module\":\"");
        out.put_json_str(m.path.as_bytes());
        out.put(b"\",\"offset\":");
        out.put_u64(off);
        if let Some(id) = &m.build_id {
            out.put(b",\"build_id\":\"");
            out.put_json_str(id.as_bytes());
            out.put(b"\"");
        }
        out.put(b"}");
    }
    out.put(b"]");
    out.put(&st.tail);
}

extern "C" fn handler(sig: libc::c_int, _info: *mut libc::siginfo_t, _ctx: *mut c_void) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(st) = STATE.get() else { return };
    let name = SIGNALS
        .iter()
        .find(|(s, _)| *s == sig)
        .map_or("SIGNAL", |(_, n)| n);

    let mut frames = [std::ptr::null_mut::<c_void>(); MAX_FRAMES + SKIP + 8];
    // SAFETY: the array has the number of slots we pass.
    let n = unsafe { libc::backtrace(frames.as_mut_ptr(), frames.len() as libc::c_int) };
    let mut ips = [0u64; MAX_FRAMES + SKIP + 8];
    let n = n.max(0) as usize;
    for i in 0..n {
        ips[i] = frames[i] as u64;
    }
    let mut out = Out::new();
    build(&mut out, st, name, &ips[SKIP.min(n)..n]);

    // SAFETY: plain syscalls on a socket we create and close here; `addr` is
    // a fully initialised sockaddr_un built at install time.
    unsafe {
        let fd = libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        );
        if fd >= 0 {
            let rc = libc::connect(
                fd,
                &st.addr as *const _ as *const libc::sockaddr,
                st.addr_len,
            );
            if rc == 0 {
                let mut sent = 0usize;
                while sent < out.len {
                    let w = libc::write(
                        fd,
                        out.buf.as_ptr().add(sent) as *const c_void,
                        out.len - sent,
                    );
                    if w <= 0 {
                        break;
                    }
                    sent += w as usize;
                }
            }
            libc::close(fd);
        }
    }
    // SA_RESETHAND restored the default action; returning re-runs the fault
    // (or, for SIGABRT, the abort), so the process dies as it would have.
}

/// Install the handlers. Idempotent; returns false when it could not (no
/// socket path that fits `sockaddr_un`, or already installed elsewhere).
pub fn install(app: &str, version: Option<&str>, socket: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let path = socket.as_os_str().as_bytes();
    // SAFETY: zeroed sockaddr_un is a valid all-zero value.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if path.is_empty() || path.len() >= addr.sun_path.len() {
        return false;
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (i, b) in path.iter().enumerate() {
        addr.sun_path[i] = *b as libc::c_char;
    }
    let addr_len = (std::mem::size_of::<libc::sa_family_t>() + path.len() + 1) as libc::socklen_t;

    let layout = Layout::current();
    let exe_id = std::env::current_exe().ok().and_then(|e| {
        let e = e.to_str()?.to_string();
        layout
            .modules
            .iter()
            .find(|m| m.path == e)
            .and_then(|m| m.build_id.clone())
    });
    let (head, middle, tail) = render_pieces(app, version, exe_id.as_deref());
    let state = State {
        head: head.into_bytes(),
        middle: middle.into_bytes(),
        tail: tail.into_bytes(),
        addr,
        addr_len,
        layout,
    };
    if STATE.set(state).is_err() {
        return false;
    }
    // Pre-warm: the first call may allocate or dlopen libgcc.
    let mut warm = [std::ptr::null_mut::<c_void>(); 8];
    // SAFETY: slots match the count passed.
    unsafe { libc::backtrace(warm.as_mut_ptr(), warm.len() as libc::c_int) };

    // SAFETY: standard sigaltstack/sigaction setup with owned, leaked memory.
    unsafe {
        let stack = libc::mmap(
            std::ptr::null_mut(),
            ALT_STACK,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        if stack != libc::MAP_FAILED {
            let ss = libc::stack_t {
                ss_sp: stack,
                ss_flags: 0,
                ss_size: ALT_STACK,
            };
            libc::sigaltstack(&ss, std::ptr::null_mut());
        }
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as *const () as usize;
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK | libc::SA_RESETHAND | libc::SA_NODEFER;
        libc::sigemptyset(&mut sa.sa_mask);
        for (sig, _) in SIGNALS {
            libc::sigaction(*sig, &sa, std::ptr::null_mut());
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bug::maps::{Module, Region};

    fn state() -> State {
        let (h, m, t) = render_pieces("clip", Some("0.3"), Some("aabb"));
        State {
            head: h.into_bytes(),
            middle: m.into_bytes(),
            tail: t.into_bytes(),
            addr: unsafe { std::mem::zeroed() },
            addr_len: 0,
            layout: Layout {
                modules: vec![
                    Module {
                        path: "/usr/bin/app".into(),
                        build_id: Some("aabbccdd".into()),
                        base: 0x1000,
                        is_dyn: true,
                    },
                    Module {
                        path: "/lib/we\"ird\\.so".into(),
                        build_id: None,
                        base: 0x9000,
                        is_dyn: true,
                    },
                ],
                regions: vec![
                    Region {
                        start: 0x2000,
                        end: 0x3000,
                        module: 0,
                    },
                    Region {
                        start: 0xa000,
                        end: 0xb000,
                        module: 1,
                    },
                ],
            },
        }
    }

    #[test]
    fn the_prerendered_request_is_valid_json_with_the_right_fields() {
        let st = state();
        let mut out = Out::new();
        build(&mut out, &st, "SIGSEGV", &[0x2100, 0xdead, 0xa100]);
        let text = std::str::from_utf8(&out.buf[..out.len]).unwrap();
        assert!(text.ends_with('\n'));
        let v: serde_json::Value = serde_json::from_str(text.trim_end()).expect("valid JSON");
        assert_eq!(v["method"], "bug.marker");
        let m = &v["params"]["marker"];
        assert_eq!(
            (m["kind"].as_str(), m["signal"].as_str()),
            (Some("crash"), Some("SIGSEGV"))
        );
        assert_eq!(v["params"]["app"], "clip");
        assert_eq!(m["app_version"], "0.3");
        let frames = m["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 2, "unknown ip skipped");
        assert_eq!(
            frames[0],
            serde_json::json!({"module": "/usr/bin/app", "offset": 0x1100, "build_id": "aabbccdd"})
        );
        // quote and backslash in a path cannot break the JSON
        assert_eq!(frames[1]["module"], "/lib/weird.so");
        assert!(frames[1].get("build_id").is_none());
    }

    #[test]
    fn output_is_bounded_and_stays_valid_json_when_full() {
        let mut st = state();
        st.layout.modules[0].path = format!("/{}", "a".repeat(2000));
        let ips: Vec<u64> = (0..200).map(|_| 0x2100).collect();
        let mut out = Out::new();
        build(&mut out, &st, "SIGABRT", &ips);
        assert!(out.len <= BUF);
        let text = std::str::from_utf8(&out.buf[..out.len]).unwrap();
        let v: serde_json::Value = serde_json::from_str(text.trim_end()).expect("still valid JSON");
        assert!(!v["params"]["marker"]["frames"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn integer_formatting() {
        let mut o = Out::new();
        o.put_u64(0);
        o.put(b",");
        o.put_u64(1234567890123456789);
        o.put(b",");
        o.put_u64(u64::MAX);
        assert_eq!(
            &o.buf[..o.len],
            b"0,1234567890123456789,18446744073709551615"
        );
    }

    #[test]
    fn rejects_sockets_paths_too_long_for_sockaddr_un() {
        let long = std::path::PathBuf::from(format!("/{}", "x".repeat(200)));
        assert!(!install("clip", None, &long));
        assert!(!install("clip", None, Path::new("")));
    }
}
