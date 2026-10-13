//! Native crash reporting: a signal handler for SIGSEGV, SIGBUS, SIGILL,
//! SIGFPE and SIGABRT that sends a `crash` marker (signal name + stack as
//! module + offset + build-id) to breadd, then restores the prior action and
//! lets the signal take its course.
//!
//! This is the in-process option from BREAD-17 (the other being
//! systemd-coredump). It needs no privileges or system configuration and works
//! in any user session. Frames only: no memory, no registers beyond the
//! instruction and frame pointers, and no environment is ever read or sent.
//!
//! # Fatal semantics and chaining
//!
//! Install saves the previous `sigaction` for each of the five signals and
//! checks every syscall. If any step fails, the actions already replaced are
//! restored, a warning is logged, and [`install`] returns `false`. Nothing is
//! left half-installed.
//!
//! When a signal arrives the handler reports it (if reporting is active), then
//! puts back the exact prior action:
//!
//! * a **synchronous fault** (`si_code > 0`, e.g. a bad load): returning
//!   re-executes the faulting instruction under the restored action, so the
//!   default action terminates the process with the same signal. A prior
//!   handler runs once.
//! * a **signal the process sent to itself** (`si_code <= 0`, e.g.
//!   `raise(SIGSEGV)`, `kill`, or `abort()`): returning would not deliver it
//!   again, so the handler calls `raise` itself. The default action terminates,
//!   a prior handler runs once, and an ignored signal stays ignored.
//!
//! # Alternate stack
//!
//! The handler needs an alternate stack so stack overflows can be reported. If
//! the installing thread already has one (for example, another library set it),
//! it is kept and not replaced; the handler then runs on that stack. Otherwise
//! a 128 KiB stack is mapped and installed. Only the installing thread gets
//! it. Other threads need their own stack to report a stack overflow.
//!
//! # Signal safety
//!
//! Everything the handler touches is prepared at install time. Inside the
//! handler only async-signal-safe calls are made: `sigaction`, `raise`,
//! `socket`, `connect`, `send`, `close`, and `write`/`read` on a pre-made pipe.
//! It allocates nothing, takes no locks, and formats into a fixed 16 KiB stack
//! buffer. It never touches `malloc`, the `log` crate, or `std::io`.
//!
//! `backtrace(3)` is deliberately not used here. Its unwinder may `dlopen`
//! libgcc and takes the loader lock, so a fault inside `malloc` or the loader
//! would deadlock or corrupt state. Pre-warming it only helps the first call,
//! not the lock. Normal code in [`panic`](super::panic) still uses it.
//!
//! Frames come from a bounded frame-pointer walk on x86_64: the faulting
//! instruction pointer, then saved frame pointers, at most [`MAX_FRAMES`]
//! entries. A frame pointer is never dereferenced directly. Each word is copied
//! with `write(2)` into a pre-made pipe, which fails with `EFAULT` on an
//! unmapped address instead of faulting, so a corrupt stack ends the walk
//! without crashing the handler. Rust code built without frame pointers gives
//! short stacks; build with `-C force-frame-pointers=yes` for full ones. On
//! other architectures the stack is only the faulting instruction.
//!
//! The module table is a snapshot taken at install time, with paths redacted
//! already. Libraries loaded later are omitted.

use std::ffi::c_void;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use super::maps::Layout;
use super::scrub;

const SIGNALS: &[(libc::c_int, &str)] = &[
    (libc::SIGSEGV, "SIGSEGV"),
    (libc::SIGBUS, "SIGBUS"),
    (libc::SIGILL, "SIGILL"),
    (libc::SIGFPE, "SIGFPE"),
    (libc::SIGABRT, "SIGABRT"),
];
const BUF: usize = 16 * 1024;
/// Most frames reported.
const MAX_FRAMES: usize = 48;
/// A frame pointer may not jump further than this up the stack.
const MAX_FRAME_GAP: usize = 1 << 20;
const ALT_STACK: usize = 128 * 1024;

/// What the report says. Prepared at install time and leaked when replaced,
/// so a handler that is already running keeps a valid pointer.
struct Report {
    head: Vec<u8>,
    middle: Vec<u8>,
    tail: Vec<u8>,
    addr: libc::sockaddr_un,
    addr_len: libc::socklen_t,
    layout: Layout,
}

/// Process-wide handler state, created once and never freed. It is complete
/// before any handler is installed, so the handler can read it without
/// locking.
struct Resources {
    prev: [libc::sigaction; SIGNALS.len()],
    pipe_r: libc::c_int,
    pipe_w: libc::c_int,
}

static RESOURCES: OnceLock<Resources> = OnceLock::new();
static REPORT: AtomicPtr<Report> = AtomicPtr::new(ptr::null_mut());
static ACTIVE: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);
static INSTALL_LOCK: Mutex<()> = Mutex::new(());

fn install_lock() -> MutexGuard<'static, ()> {
    INSTALL_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

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
fn build(out: &mut Out, rep: &Report, signal: &str, ips: &[u64]) {
    out.put(&rep.head);
    out.put(b"\"");
    out.put(signal.as_bytes());
    out.put(b"\"");
    out.put(&rep.middle);
    out.put(b"[");
    let mut first = true;
    for ip in ips.iter().take(MAX_FRAMES) {
        let Some((m, off)) = rep.layout.locate(*ip) else {
            continue;
        };
        // Whole frames only: leave room for this frame (worst case), the
        // separator, the closing bracket and the tail, or stop here.
        let need =
            m.path.len() + m.build_id.as_ref().map_or(0, String::len) + 80 + 1 + 1 + rep.tail.len();
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
    out.put(&rep.tail);
}

/// Copy two machine words from `addr` without faulting. `write(2)` copies
/// from user memory and returns `EFAULT` for an unmapped address, so a bad
/// frame pointer ends the walk instead of crashing the handler.
///
/// # Safety
/// `res` must be the installed resources (its pipe is non-blocking).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn read_pair(res: &Resources, addr: usize, out: &mut [usize; 2]) -> bool {
    const N: usize = 2 * std::mem::size_of::<usize>();
    if libc::write(res.pipe_w, addr as *const c_void, N) != N as isize {
        return false;
    }
    let mut got = 0usize;
    while got < N {
        let r = libc::read(
            res.pipe_r,
            (out.as_mut_ptr() as *mut u8).add(got) as *mut c_void,
            N - got,
        );
        if r <= 0 {
            return false;
        }
        got += r as usize;
    }
    true
}

/// Instruction pointer of the fault, then return addresses from the saved
/// frame pointer chain (x86_64 SysV layout: `[rbp] = caller's rbp`,
/// `[rbp + 8] = return address`).
///
/// # Safety
/// `ctx` must be the kernel-provided `ucontext_t` for the current signal.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn walk(res: &Resources, ctx: *mut c_void, out: &mut [u64; MAX_FRAMES]) -> usize {
    // glibc's sys/ucontext.h indices for x86_64 general registers.
    const REG_RBP: usize = 10;
    const REG_RIP: usize = 16;
    if ctx.is_null() {
        return 0;
    }
    let uc = ctx as *const libc::ucontext_t;
    let gregs = &(*uc).uc_mcontext.gregs;
    let mut n = 0;
    let rip = gregs[REG_RIP] as u64;
    if rip != 0 {
        out[n] = rip;
        n += 1;
    }
    let mut fp = gregs[REG_RBP] as usize;
    while n < MAX_FRAMES && fp != 0 && fp % std::mem::align_of::<usize>() == 0 {
        let mut pair = [0usize; 2];
        if !read_pair(res, fp, &mut pair) {
            break;
        }
        let (next, ret) = (pair[0], pair[1]);
        if ret == 0 {
            break;
        }
        out[n] = ret as u64;
        n += 1;
        if next <= fp || next - fp > MAX_FRAME_GAP {
            break;
        }
        fp = next;
    }
    n
}

/// # Safety
/// Same contract as the x86_64 version; this one never reads memory.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
unsafe fn walk(_res: &Resources, _ctx: *mut c_void, _out: &mut [u64; MAX_FRAMES]) -> usize {
    0
}

fn send_report(rep: &Report, signal: &str, frames: &[u64]) {
    let mut out = Out::new();
    build(&mut out, rep, signal, frames);
    // SAFETY: plain syscalls on a socket made and closed here. `addr` is a
    // fully initialised sockaddr_un. MSG_NOSIGNAL keeps a closed peer from
    // raising SIGPIPE, which would replace the signal being reported.
    unsafe {
        let fd = libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        );
        if fd < 0 {
            return;
        }
        if libc::connect(fd, &rep.addr as *const _ as *const libc::sockaddr, rep.addr_len) == 0 {
            let mut sent = 0usize;
            while sent < out.len {
                let w = libc::send(
                    fd,
                    out.buf.as_ptr().add(sent) as *const c_void,
                    out.len - sent,
                    libc::MSG_NOSIGNAL,
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

extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, ctx: *mut c_void) {
    let Some(idx) = SIGNALS.iter().position(|(s, _)| *s == sig) else {
        return;
    };
    let Some(res) = RESOURCES.get() else {
        // Unreachable once installed (resources are set first), but if it
        // happens there is no prior action to chain to.
        default_action(sig);
        return;
    };

    if ACTIVE.load(Ordering::Acquire) && !RUNNING.swap(true, Ordering::AcqRel) {
        let rep = REPORT.load(Ordering::Acquire);
        if !rep.is_null() {
            // SAFETY: REPORT points to a leaked Report that is never freed.
            let rep = unsafe { &*rep };
            let mut frames = [0u64; MAX_FRAMES];
            // SAFETY: `ctx` is the kernel-provided ucontext for this signal.
            let n = unsafe { walk(res, ctx, &mut frames) };
            send_report(rep, SIGNALS[idx].1, &frames[..n]);
        }
        RUNNING.store(false, Ordering::Release);
    }

    let si_code = if info.is_null() {
        0
    } else {
        // SAFETY: the kernel passes a valid siginfo_t with SA_SIGINFO.
        unsafe { (*info).si_code }
    };
    let prev = &res.prev[idx];
    if prev.sa_sigaction == libc::SIG_IGN {
        // Returning from a synchronous fault under SIG_IGN would retry the
        // instruction forever. Terminate as the kernel would have.
        if si_code > 0 {
            default_action(sig);
        }
        return;
    }
    if prev.sa_sigaction == libc::SIG_DFL {
        // SAFETY: restoring the default saved at install time.
        unsafe { libc::sigaction(sig, prev, ptr::null_mut()) };
        if si_code <= 0 {
            // Sent by the process itself: returning would not deliver it
            // again. Deliver it now so the default action runs.
            // SAFETY: raise is async-signal-safe.
            unsafe { libc::raise(sig) };
        }
        // Otherwise a synchronous fault: returning re-executes the faulting
        // instruction under the default action, which terminates.
        return;
    }
    // A prior handler is installed. Run it directly, with the same arguments
    // the kernel gave us, so it sees the original fault.
    if prev.sa_flags & libc::SA_SIGINFO != 0 {
        // SAFETY: SA_SIGINFO handlers take (sig, info, ucontext) and the
        // function pointer was stored by the kernel-facing sigaction call.
        let f: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut c_void) =
            unsafe { std::mem::transmute(prev.sa_sigaction) };
        f(sig, info, ctx);
    } else {
        // SAFETY: a plain handler takes one signal number.
        let f: extern "C" fn(libc::c_int) = unsafe { std::mem::transmute(prev.sa_sigaction) };
        f(sig);
    }
}

/// Put back the default action for `sig` and deliver it again.
fn default_action(sig: libc::c_int) {
    // SAFETY: sigaction and raise are async-signal-safe; a zeroed sigaction
    // with SIG_DFL is a valid value.
    unsafe {
        let mut dfl: libc::sigaction = std::mem::zeroed();
        dfl.sa_sigaction = libc::SIG_DFL;
        libc::sigaction(sig, &dfl, ptr::null_mut());
        libc::raise(sig);
    }
}

/// Read the current action for `sig`; `None` if the syscall fails.
fn query(sig: libc::c_int) -> Option<libc::sigaction> {
    // SAFETY: a zeroed sigaction is a valid out-parameter.
    let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: a null `act` only reads the current action.
    let rc = unsafe { libc::sigaction(sig, ptr::null(), &mut old) };
    (rc == 0).then_some(old)
}

/// Build the process-wide resources: prior actions and a pipe for the frame
/// walk. Creates nothing that outlives a failure, and changes no signal state.
fn make_resources() -> Option<Resources> {
    let mut prev: [libc::sigaction; SIGNALS.len()] = unsafe { std::mem::zeroed() };
    for (i, (sig, _)) in SIGNALS.iter().enumerate() {
        prev[i] = query(*sig)?;
    }
    let mut fds = [-1 as libc::c_int; 2];
    // SAFETY: pipe2 writes two fds into the array on success.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } != 0 {
        return None;
    }
    Some(Resources {
        prev,
        pipe_r: fds[0],
        pipe_w: fds[1],
    })
}

/// Outcome of [`ensure_alt_stack`].
enum AltStack {
    /// The thread already had an alternate stack; it is kept as is.
    Existing,
    /// A new stack was mapped and installed at this address.
    Mapped(*mut c_void),
}

/// Make the calling thread's alternate stack ready. An existing stack is
/// kept, never replaced. On failure nothing is left mapped or installed.
fn ensure_alt_stack() -> Option<AltStack> {
    // SAFETY: a zeroed stack_t is a valid out-parameter.
    let mut cur: libc::stack_t = unsafe { std::mem::zeroed() };
    // SAFETY: a null `ss` only reads the current stack.
    if unsafe { libc::sigaltstack(ptr::null(), &mut cur) } != 0 {
        return None;
    }
    // Keep an existing stack only if it can hold the handler's buffers.
    if cur.ss_flags & libc::SS_DISABLE == 0 && cur.ss_size >= ALT_STACK {
        return Some(AltStack::Existing);
    }
    // SAFETY: anonymous private mapping, owned here.
    let mem = unsafe {
        libc::mmap(
            ptr::null_mut(),
            ALT_STACK,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if mem == libc::MAP_FAILED {
        return None;
    }
    let new = libc::stack_t {
        ss_sp: mem,
        ss_flags: 0,
        ss_size: ALT_STACK,
    };
    // SAFETY: installing a stack we just mapped.
    if unsafe { libc::sigaltstack(&new, ptr::null_mut()) } != 0 {
        // SAFETY: nothing uses the mapping, so it can go.
        unsafe { libc::munmap(mem, ALT_STACK) };
        return None;
    }
    Some(AltStack::Mapped(mem))
}

/// Install the handlers and start reporting. Returns `false` (and logs why)
/// when setup could not be completed; in that case every signal action is
/// as it was before the call. Idempotent: a later call replaces what is
/// reported (app, version, socket) and keeps the first install's prior
/// actions.
pub fn install(app: &str, version: Option<&str>, socket: &Path) -> bool {
    let _lock = install_lock();
    let Some((addr, addr_len)) = super::sockaddr_for(socket) else {
        log::warn!("breadbug: native crash handler not installed: socket path unusable");
        return false;
    };

    let scrub = scrub::process_default();
    let raw_layout = Layout::current();
    let exe_id = std::env::current_exe().ok().and_then(|e| {
        let e = e.to_str()?.to_string();
        raw_layout
            .modules
            .iter()
            .find(|m| m.path == e)
            .and_then(|m| m.build_id.clone())
    });
    let layout = Layout {
        modules: raw_layout
            .modules
            .into_iter()
            .map(|mut m| {
                m.path = scrub.redact(&m.path);
                m
            })
            .collect(),
        regions: raw_layout.regions,
    };
    let app = scrub.redact(app);
    let version = version.map(|v| scrub.redact(v));
    let (head, middle, tail) = render_pieces(&app, version.as_deref(), exe_id.as_deref());
    let report = Box::into_raw(Box::new(Report {
        head: head.into_bytes(),
        middle: middle.into_bytes(),
        tail: tail.into_bytes(),
        addr,
        addr_len,
        layout,
    }));

    if RESOURCES.get().is_none() {
        let Some(res) = make_resources() else {
            log::warn!("breadbug: could not read signal actions or make a pipe; crash reporting off");
            // SAFETY: the report was never published.
            drop(unsafe { Box::from_raw(report) });
            return false;
        };
        let _ = RESOURCES.set(res);
    }
    let res = RESOURCES.get().expect("set above");

    let stack = match ensure_alt_stack() {
        Some(s) => s,
        None => {
            log::warn!("breadbug: alternate signal stack setup failed; crash reporting off");
            // SAFETY: the report was never published.
            drop(unsafe { Box::from_raw(report) });
            return false;
        }
    };

    // Publish the report before any handler can run, so a crash right after
    // installation is reported. The old report is leaked on purpose: a
    // handler may still be reading it.
    REPORT.store(report, Ordering::Release);

    // SAFETY: an all-zero sigaction is valid; the fields below are set next.
    let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
    sa.sa_sigaction = handler as *const () as usize;
    sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK | libc::SA_NODEFER;
    // SAFETY: sigemptyset writes an empty mask into owned storage.
    unsafe { libc::sigemptyset(&mut sa.sa_mask) };
    for (i, (sig, _)) in SIGNALS.iter().enumerate() {
        // SAFETY: installing our handler; `prev[i]` already holds the action
        // that a rollback restores.
        if unsafe { libc::sigaction(*sig, &sa, ptr::null_mut()) } != 0 {
            for (j, (done, _)) in SIGNALS[..i].iter().enumerate() {
                // SAFETY: restoring the actions saved at first install.
                unsafe { libc::sigaction(*done, &res.prev[j], ptr::null_mut()) };
            }
            log::warn!("breadbug: sigaction failed for signal {sig}; crash reporting off");
            ACTIVE.store(false, Ordering::Release);
            if let AltStack::Mapped(mem) = stack {
                // SAFETY: disable the stack before unmapping it, so the thread
                // never points at freed memory.
                unsafe {
                    let off = libc::stack_t {
                        ss_sp: ptr::null_mut(),
                        ss_flags: libc::SS_DISABLE,
                        ss_size: 0,
                    };
                    libc::sigaltstack(&off, ptr::null_mut());
                    libc::munmap(mem, ALT_STACK);
                }
            }
            return false;
        }
    }
    ACTIVE.store(true, Ordering::Release);
    true
}

/// Stop reporting. The handlers stay installed but pass the signal through
/// to the prior action, so a crash still behaves as it did before.
pub fn disable() {
    ACTIVE.store(false, Ordering::Release);
}

/// Whether crash reporting is currently active in this process.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bug::maps::{Module, Region};

    fn report() -> Report {
        let (h, m, t) = render_pieces("clip", Some("0.3"), Some("aabb"));
        Report {
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
        let rep = report();
        let mut out = Out::new();
        build(&mut out, &rep, "SIGSEGV", &[0x2100, 0xdead, 0xa100]);
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
        let mut rep = report();
        rep.layout.modules[0].path = format!("/{}", "a".repeat(2000));
        let ips: Vec<u64> = (0..200).map(|_| 0x2100).collect();
        let mut out = Out::new();
        build(&mut out, &rep, "SIGABRT", &ips);
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

    #[test]
    fn module_paths_are_redacted_before_they_are_rendered() {
        let mut rep = report();
        let s = scrub::Scrubber::new(Some("/home/someone"), Some("someone"));
        rep.layout.modules[0].path = s.redact("/home/someone/bin/app");
        let mut out = Out::new();
        build(&mut out, &rep, "SIGSEGV", &[0x2100]);
        let text = std::str::from_utf8(&out.buf[..out.len]).unwrap();
        assert!(!text.contains("someone"), "{text}");
        assert!(text.contains("~/bin/app"));
    }
}
