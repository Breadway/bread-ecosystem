//! breadbug integration for sibling apps: crash, freeze and log reporting to
//! breadd.
//!
//! Reporting is on by default. [`BreadClient::connect`] starts it for the
//! process, so adopting it is a dependency bump, not per-app code:
//!
//! ```ignore
//! let client = BreadClient::connect("clip"); // panic hook, crash handlers, logs, heartbeat
//! // Non-GTK apps with a main loop call this once per iteration. GTK apps
//! // with the `gtk` feature need nothing: the heartbeat ticks from GLib.
//! bread_utils::bug::tick();
//! ```
//!
//! Apps that handle sensitive data call
//! [`BreadClient::connect_without_bug_reporting`]. Apps that need other
//! options call [`BreadClient::enable_bug_reporting`] with a [`BugOptions`].
//! `BREAD_BUG_REPORTING=0` in the environment also turns the default off.
//!
//! What it installs:
//!
//! * a **panic hook** that sends the message, location and a stack of
//!   `(module, offset, build-id)` frames (see [`panic`]);
//! * **native crash handlers** for SIGSEGV/SIGBUS/SIGILL/SIGFPE/SIGABRT that
//!   chain to whatever was installed before (see [`native`]);
//! * a **log forwarder**: a bounded queue that drops the oldest lines under
//!   backpressure, sends at a sustained rate of [`LOG_RATE_PER_SEC`] lines
//!   per second, and never blocks the caller;
//! * a **heartbeat** so breadd can tell when the main loop has frozen.
//!
//! Redaction happens in the client before anything reaches the socket: home
//! directories, user names, e-mail addresses, IPv4 addresses, token-looking
//! strings and long hex identifiers are replaced (see [`scrub`]). The client
//! never sends environment variables. This is the first of three layers:
//! breadd scrubs with an allowlist on ingest, and the report is scrubbed again
//! at send time (ECO-66, "scrub in two places"). The client-side layer is a
//! denylist, so it is not the last line of defence.
//!
//! Everything is best effort and degrades to a no-op when breadd is missing:
//! constructing a client never fails, nothing blocks the app, and connects
//! to a full or absent socket fail at once instead of waiting.
//!
//! Build requirement: release binaries should carry a GNU build-id
//! (`RUSTFLAGS="-C link-arg=-Wl,--build-id"`) so crashes can be symbolicated
//! server-side; [`has_build_id`] reports whether the running binary has one.
//! Frame pointers (`-C force-frame-pointers=yes`) give complete native stacks.
//!
//! The measured cost of [`BugGuard::tick`] and [`BugGuard::log`] is in
//! `docs/breadbug-client.md`; reproduce it with
//! `cargo run --release -p bread-utils --features bread-client --example bug_overhead`.

pub mod elf;
pub mod maps;
pub mod native;
pub mod panic;
pub mod scrub;

use crossbeam_queue::ArrayQueue;
use std::fmt::Write as _;
use std::io::Write;
use std::os::unix::io::FromRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::bread_client::BreadClient;
use scrub::Scrubber;

/// Lines buffered for forwarding; the oldest are dropped beyond this.
const LOG_QUEUE: usize = 512;
/// Most lines in one `bug.logs` request.
const LOG_BATCH: usize = 50;
/// Sustained log lines per second sent to breadd.
pub const LOG_RATE_PER_SEC: u64 = 50;
/// Lines that may be sent in one burst before the sustained rate applies.
const LOG_BURST: u64 = 200;
/// Longest log message forwarded, in bytes.
const MAX_LOG_BYTES: usize = 2048;
/// How long dropping a guard waits for its threads to finish.
const JOIN_TIMEOUT: Duration = Duration::from_millis(500);
/// Write timeout for one request to breadd.
const WRITE_TIMEOUT: Duration = Duration::from_millis(200);
/// Environment variable that turns the default (automatic) reporting off.
const OPT_OUT_ENV: &str = "BREAD_BUG_REPORTING";

/// What to enable and how.
#[derive(Debug, Clone)]
pub struct BugOptions {
    /// App version string shown in reports (`env!("CARGO_PKG_VERSION")`).
    pub version: Option<String>,
    /// Install the Rust panic hook.
    pub panic_hook: bool,
    /// Include the panic message in the report. Turn off for apps that
    /// handle private text (a clipboard manager, a search index): a
    /// formatted panic message can contain user data. The location and
    /// stack are always kept.
    pub panic_message: bool,
    /// Install native crash signal handlers.
    pub native_crashes: bool,
    /// Forward [`BugGuard::log`] / [`BugLogger`] lines to breadd.
    pub forward_logs: bool,
    /// Send heartbeats for freeze detection.
    pub heartbeat: bool,
    /// How often heartbeats are sent.
    pub heartbeat_interval: Duration,
    /// Send heartbeats even if [`BugGuard::tick`] is never called. Only for
    /// apps with no main loop to tick from; it cannot detect a frozen loop.
    pub heartbeat_unconditional: bool,
    /// Socket to talk to; `None` uses breadd's default location.
    pub socket: Option<PathBuf>,
}

impl BugOptions {
    /// Everything on, reporting the app version `version`.
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: Some(version.into()),
            ..Self::automatic()
        }
    }

    /// What [`BreadClient::connect`] turns on: everything on, no version
    /// (the client does not know the app's version).
    pub fn automatic() -> Self {
        Self {
            version: None,
            panic_hook: true,
            panic_message: true,
            native_crashes: true,
            forward_logs: true,
            heartbeat: true,
            heartbeat_interval: Duration::from_secs(2),
            heartbeat_unconditional: false,
            socket: None,
        }
    }

    /// Everything off: for apps that handle sensitive data and want no
    /// automatic capture at all. (Users can also exclude an app in
    /// `breadbug.toml`; breadd then stores nothing for it.)
    pub fn disabled() -> Self {
        Self {
            version: None,
            panic_hook: false,
            panic_message: false,
            native_crashes: false,
            forward_logs: false,
            heartbeat: false,
            heartbeat_interval: Duration::from_secs(2),
            heartbeat_unconditional: false,
            socket: None,
        }
    }
}

/// Whether the running executable has a GNU build-id.
pub fn has_build_id() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| elf::read(&p))
        .is_some_and(|i| i.build_id.is_some())
}

/// The `sockaddr_un` for `path`, or `None` if it is empty or too long.
pub(crate) fn sockaddr_for(path: &Path) -> Option<(libc::sockaddr_un, libc::socklen_t)> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: an all-zero sockaddr_un is a valid value.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.is_empty() || bytes.len() >= addr.sun_path.len() {
        return None;
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, b) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = *b as libc::c_char;
    }
    let len = std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1;
    Some((addr, len as libc::socklen_t))
}

/// Connect to a unix socket without waiting: a full listen backlog fails at
/// once (EAGAIN) instead of blocking the caller.
pub(crate) fn connect_unix(addr: &libc::sockaddr_un, len: libc::socklen_t) -> Option<UnixStream> {
    // SAFETY: plain socket and connect calls. The fd is closed on failure and
    // otherwise owned by the returned stream.
    unsafe {
        let fd = libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        );
        if fd < 0 {
            return None;
        }
        if libc::connect(fd, addr as *const _ as *const libc::sockaddr, len) != 0 {
            libc::close(fd);
            return None;
        }
        let stream = UnixStream::from_raw_fd(fd);
        stream.set_nonblocking(false).ok()?;
        Some(stream)
    }
}

/// Best-effort single request: connect, write one line, close. Returns
/// whether it was written. Never blocks longer than the write timeout.
pub(crate) fn send_request(socket: &Path, method: &str, params: Value) -> bool {
    let line = json!({ "id": "0", "method": method, "params": params }).to_string();
    let Some((addr, len)) = sockaddr_for(socket) else {
        return false;
    };
    let Some(mut stream) = connect_unix(&addr, len) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    writeln!(stream, "{line}").is_ok()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

struct Line {
    level: &'static str,
    msg: String,
    t: u64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Bounded lock-free log queue. Producers never wait for the consumer: a full
/// queue evicts its oldest line before retrying the push.
struct Queue {
    lines: ArrayQueue<Line>,
    wait_lock: Mutex<()>,
    ready: Condvar,
}

impl Queue {
    fn new() -> Self {
        Self {
            lines: ArrayQueue::new(LOG_QUEUE),
            wait_lock: Mutex::new(()),
            ready: Condvar::new(),
        }
    }

    /// Queue `line`. Returns how many lines were evicted to make room.
    fn push(&self, line: Line) -> u64 {
        let mut dropped = 0;
        let mut pending = line;
        loop {
            match self.lines.push(pending) {
                Ok(()) => {
                    self.ready.notify_one();
                    return dropped;
                }
                Err(line) => {
                    pending = line;
                    if let Some(oldest) = self.lines.pop() {
                        drop(oldest);
                        dropped += 1;
                    }
                }
            }
        }
    }

    /// Up to `max` lines, waiting at most `wait` for the first one. The
    /// output buffer is reserved before locking, so no allocation happens
    /// while producers may be waiting on the lock.
    fn take(&self, max: usize, wait: Duration) -> Vec<Line> {
        let mut out = Vec::with_capacity(max.min(LOG_QUEUE));
        if self.lines.is_empty() && !wait.is_zero() {
            let guard = self.ready_lock();
            if self.lines.is_empty() {
                let _ = self.ready.wait_timeout(guard, wait);
            }
        }
        for _ in 0..max {
            let Some(line) = self.lines.pop() else { break };
            out.push(line);
        }
        out
    }

    /// Throw away everything queued; returns how many lines that was.
    fn discard_all(&self) -> u64 {
        let mut n = 0;
        while self.lines.pop().is_some() {
            n += 1;
        }
        n
    }

    fn ready_lock(&self) -> MutexGuard<'_, ()> {
        self.wait_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

struct Shared {
    app: String,
    socket: PathBuf,
    scrub: Scrubber,
    forward: bool,
    interval: Duration,
    unconditional: bool,
    /// Milliseconds since `epoch` of the last [`BugGuard::tick`].
    last_tick: AtomicU64,
    epoch: Instant,
    stop: AtomicBool,
    dropped_logs: AtomicU64,
    queue: Queue,
}

/// Keeps the reporting threads alive; dropping it announces a clean exit to
/// the freeze watchdog and stops them. Dropping never waits longer than
/// [`JOIN_TIMEOUT`], even while a [`BugLogger`] clone is still alive.
pub struct BugGuard {
    shared: Arc<Shared>,
    done_tx: Option<Sender<()>>,
    done_rx: Receiver<()>,
    threads: Vec<JoinHandle<()>>,
    heartbeat_on: bool,
    #[cfg(feature = "gtk")]
    glib_source: Option<gtk4::glib::SourceId>,
}

/// The process's live reporting guard. Holds the default started by
/// [`BreadClient::connect`], or whatever [`BreadClient::enable_bug_reporting`]
/// replaced it with. Only one set of reporting threads runs at a time.
static LIVE: Mutex<Option<BugGuard>> = Mutex::new(None);

pub(crate) fn automatic_allowed(env: Option<&str>) -> bool {
    !matches!(
        env.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "off" | "false" | "no" | "disabled")
    )
}

/// Start the default reporting for the process, unless something is already
/// running or the user opted out with `BREAD_BUG_REPORTING=0`.
pub(crate) fn start_automatic(app: &str) {
    if !automatic_allowed(std::env::var(OPT_OUT_ENV).ok().as_deref()) {
        return;
    }
    let mut live = lock(&LIVE);
    if live.is_none() {
        *live = Some(start(app, BugOptions::automatic()));
    }
}

/// Tick the default reporting guard from the app's main loop (cheap). Does
/// nothing if the app replaced the default with
/// [`BreadClient::enable_bug_reporting`]; tick that guard instead.
/// GTK apps (feature `gtk`) do not need this: the default ticks from GLib.
pub fn tick() {
    if let Some(guard) = lock(&LIVE).as_ref() {
        guard.tick();
    }
}

impl BreadClient {
    /// Turn on crash, freeze and log reporting for this app with `opts`,
    /// replacing the default that [`BreadClient::connect`] started. The
    /// returned guard owns the new threads; the default's threads are stopped
    /// first. Never fails and never blocks: with breadd absent everything
    /// quietly does nothing. Panic and crash settings are process-wide, and
    /// the most recent call decides what they report.
    pub fn enable_bug_reporting(&self, opts: BugOptions) -> BugGuard {
        drop(lock(&LIVE).take());
        start(self.app_id(), opts)
    }
}

fn start(app: &str, opts: BugOptions) -> BugGuard {
    let socket = opts
        .socket
        .clone()
        .unwrap_or_else(bread_shared::resolve_socket_path);
    let scrub = Scrubber::from_env();

    if opts.panic_hook {
        panic::configure(panic::Config {
            app: app.to_string(),
            version: opts.version.clone(),
            socket: socket.clone(),
            include_message: opts.panic_message,
        });
    } else {
        panic::disable();
    }
    if opts.native_crashes {
        native::install(app, opts.version.as_deref(), &socket);
    } else {
        native::disable();
    }

    let shared = Arc::new(Shared {
        app: app.to_string(),
        socket,
        scrub,
        forward: opts.forward_logs,
        interval: opts.heartbeat_interval.max(Duration::from_millis(250)),
        unconditional: opts.heartbeat_unconditional,
        last_tick: AtomicU64::new(0),
        epoch: Instant::now(),
        stop: AtomicBool::new(false),
        dropped_logs: AtomicU64::new(0),
        queue: Queue::new(),
    });

    let (done_tx, done_rx) = mpsc::channel();
    let mut threads = Vec::new();
    if opts.forward_logs {
        let sh = shared.clone();
        let tx = done_tx.clone();
        if let Ok(h) = std::thread::Builder::new()
            .name("bug-logs".into())
            .spawn(move || log_loop(&sh, tx))
        {
            threads.push(h);
        }
    }
    if opts.heartbeat {
        let sh = shared.clone();
        let tx = done_tx.clone();
        if let Ok(h) = std::thread::Builder::new()
            .name("bug-heartbeat".into())
            .spawn(move || heartbeat_loop(&sh, tx))
        {
            threads.push(h);
        }
    }

    #[cfg(feature = "gtk")]
    let glib_source = opts.heartbeat.then(|| attach_glib(&shared));

    BugGuard {
        shared,
        done_tx: Some(done_tx),
        done_rx,
        threads,
        heartbeat_on: opts.heartbeat,
        #[cfg(feature = "gtk")]
        glib_source,
    }
}

/// Tick from the GLib main context: the timeout only runs while the UI
/// thread is responsive, which is exactly the signal wanted. The source is
/// removed when the guard is dropped.
#[cfg(feature = "gtk")]
fn attach_glib(shared: &Arc<Shared>) -> gtk4::glib::SourceId {
    let sh = shared.clone();
    let every = (shared.interval / 2).max(Duration::from_millis(100));
    gtk4::glib::timeout_add(every, move || {
        if sh.stop.load(Ordering::Acquire) {
            return gtk4::glib::ControlFlow::Break;
        }
        tick_shared(&sh);
        gtk4::glib::ControlFlow::Continue
    })
}

fn level_name(level: &str) -> &'static str {
    match level.to_ascii_uppercase().as_str() {
        "TRACE" => "TRACE",
        "DEBUG" => "DEBUG",
        "WARN" | "WARNING" => "WARN",
        "ERROR" | "ERR" | "FATAL" => "ERROR",
        _ => "INFO",
    }
}

fn clip(msg: &str) -> &str {
    if msg.len() <= MAX_LOG_BYTES {
        return msg;
    }
    let mut cut = MAX_LOG_BYTES;
    while !msg.is_char_boundary(cut) {
        cut -= 1;
    }
    &msg[..cut]
}

/// The text that may leave the process for a log line: clipped, then
/// redacted.
fn text_for_send(scrub: &Scrubber, msg: &str) -> String {
    scrub.redact(clip(msg))
}

/// Formats at most [`MAX_LOG_BYTES`] into a buffer, then stops formatting, so
/// a huge `Display` impl cannot allocate without bound.
struct Bounded {
    buf: String,
}

impl std::fmt::Write for Bounded {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let room = MAX_LOG_BYTES.saturating_sub(self.buf.len());
        if s.len() <= room {
            self.buf.push_str(s);
            return Ok(());
        }
        let mut cut = room;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        self.buf.push_str(&s[..cut]);
        Err(std::fmt::Error)
    }
}

fn tick_shared(sh: &Shared) {
    let ms = sh.epoch.elapsed().as_millis() as u64 + 1;
    sh.last_tick.store(ms, Ordering::Relaxed);
}

fn enqueue(sh: &Shared, level: &'static str, msg: &str) {
    if !sh.forward || sh.stop.load(Ordering::Acquire) {
        return;
    }
    let line = Line {
        level,
        msg: text_for_send(&sh.scrub, msg),
        t: now_ms(),
    };
    let dropped = sh.queue.push(line);
    if dropped > 0 {
        sh.dropped_logs.fetch_add(dropped, Ordering::Relaxed);
    }
}

impl BugGuard {
    /// Call from the app's main loop (cheap). Heartbeats are only sent while
    /// ticks keep arriving, so a frozen main loop stops them and breadd can
    /// notice. GTK apps (feature `gtk`) tick from GLib automatically.
    pub fn tick(&self) {
        tick_shared(&self.shared);
    }

    /// Queue a log line for breadd. Lossy by design: never blocks, and under
    /// backpressure the oldest queued line is dropped first.
    pub fn log(&self, level: &str, msg: &str) {
        enqueue(&self.shared, level_name(level), msg);
    }

    /// How many log lines were dropped (evicted from a full queue, or over
    /// the rate budget with breadd unreachable).
    pub fn dropped_logs(&self) -> u64 {
        self.shared.dropped_logs.load(Ordering::Relaxed)
    }

    /// A `log::Log` backend that forwards records to this guard and nothing
    /// else; chain it with your normal logger if you want both.
    pub fn logger(&self) -> BugLogger {
        BugLogger {
            shared: self.shared.clone(),
        }
    }
}

impl Drop for BugGuard {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.queue.ready.notify_all();
        #[cfg(feature = "gtk")]
        if let Some(id) = self.glib_source.take() {
            if let Some(source) = gtk4::glib::MainContext::default().find_source_by_id(&id) {
                source.destroy();
            }
        }
        if self.heartbeat_on {
            send_request(
                &self.shared.socket,
                "bug.heartbeat",
                json!({ "app": self.shared.app, "stopping": true }),
            );
        }
        // Our sender goes away; the threads' senders go when they exit, so
        // `recv` reports disconnection once every thread has finished.
        self.done_tx = None;
        let deadline = Instant::now() + JOIN_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || self.done_rx.recv_timeout(left).is_err() {
                break;
            }
        }
        for h in self.threads.drain(..) {
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }
}

/// `log::Log` adapter; see [`BugGuard::logger`].
pub struct BugLogger {
    shared: Arc<Shared>,
}

impl log::Log for BugLogger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        self.shared.forward && !self.shared.stop.load(Ordering::Acquire)
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut text = Bounded { buf: String::new() };
        let _ = write!(text, "{}: {}", record.target(), record.args());
        enqueue(&self.shared, level_name(record.level().as_str()), &text.buf);
    }

    fn flush(&self) {}
}

fn send_logs(sh: &Shared, lines: Vec<Line>, backoff_until: &mut Option<Instant>) {
    if backoff_until.is_some_and(|t| Instant::now() < t) {
        sh.dropped_logs
            .fetch_add(lines.len() as u64, Ordering::Relaxed);
        return;
    }
    let items: Vec<Value> = lines
        .iter()
        .map(|l| json!({ "level": l.level, "msg": l.msg, "t": l.t }))
        .collect();
    let ok = send_request(
        &sh.socket,
        "bug.logs",
        json!({ "app": sh.app, "lines": items }),
    );
    // With breadd down, stop hammering the socket for a while.
    *backoff_until = (!ok).then(|| Instant::now() + Duration::from_secs(5));
}

fn log_loop(sh: &Shared, done: Sender<()>) {
    let mut tokens = LOG_BURST as f64;
    let mut last = Instant::now();
    let mut backoff_until: Option<Instant> = None;
    while !sh.stop.load(Ordering::Acquire) {
        let now = Instant::now();
        tokens = (tokens + now.duration_since(last).as_secs_f64() * LOG_RATE_PER_SEC as f64)
            .min(LOG_BURST as f64);
        last = now;
        let budget = (tokens as usize).min(LOG_BATCH);
        if budget == 0 {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        let lines = sh.queue.take(budget, Duration::from_millis(100));
        if lines.is_empty() {
            continue;
        }
        tokens -= lines.len() as f64;
        send_logs(sh, lines, &mut backoff_until);
    }
    // Shutdown: one final request, then everything else is dropped. Bounded
    // by a single write timeout no matter how much is queued.
    let lines = sh.queue.take(LOG_BATCH, Duration::ZERO);
    if !lines.is_empty() {
        send_logs(sh, lines, &mut backoff_until);
    }
    let left = sh.queue.discard_all();
    sh.dropped_logs.fetch_add(left, Ordering::Relaxed);
    let _ = done.send(());
}

fn heartbeat_loop(sh: &Shared, done: Sender<()>) {
    let interval_ms = sh.interval.as_millis() as u64;
    while !sh.stop.load(Ordering::Acquire) {
        let last = sh.last_tick.load(Ordering::Relaxed);
        let now = sh.epoch.elapsed().as_millis() as u64 + 1;
        let alive = sh.unconditional || (last != 0 && now.saturating_sub(last) <= interval_ms * 2);
        if alive {
            send_request(
                &sh.socket,
                "bug.heartbeat",
                json!({ "app": sh.app, "pid": std::process::id(), "interval_ms": interval_ms }),
            );
        }
        // Sleep in small slices so shutdown is prompt.
        let until = Instant::now() + sh.interval;
        while Instant::now() < until && !sh.stop.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(25).min(sh.interval));
        }
    }
    let _ = done.send(());
}

#[cfg(test)]
mod tests;
