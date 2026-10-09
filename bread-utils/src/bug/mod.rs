//! breadbug integration for sibling apps: crash, freeze and log reporting to
//! breadd with one call.
//!
//! ```ignore
//! let client = BreadClient::connect("clip");
//! let bug = client.enable_bug_reporting(BugOptions::new(env!("CARGO_PKG_VERSION")));
//! // in the main loop (or via `bug.attach_glib_main_loop()` for GTK apps):
//! bug.tick();
//! ```
//!
//! What it installs:
//!
//! * a **panic hook** that sends the message, location and a stack of
//!   `(module, offset, build-id)` frames (see [`panic`]);
//! * **native crash handlers** for SIGSEGV/SIGBUS/SIGILL/SIGFPE/SIGABRT
//!   (see [`native`]);
//! * a **log forwarder** (bounded, lossy, batched) for [`BugGuard::log`] and
//!   the [`BugLogger`] `log` backend;
//! * a **heartbeat** so breadd can tell when the main loop has frozen.
//!
//! Everything is best effort and degrades to a no-op when breadd is missing:
//! constructing the guard never fails, nothing blocks the app, and unscrubbed
//! text goes only to breadd's local socket (breadd scrubs before storing).
//! Nothing here sends anything off the machine; that is `breadbug`, and only
//! when the user asks.
//!
//! Build requirement: release binaries should carry a GNU build-id
//! (`RUSTFLAGS="-C link-arg=-Wl,--build-id"`) so crashes can be symbolicated
//! server-side; [`has_build_id`] reports whether the running binary has one.

pub mod elf;
pub mod maps;
pub mod native;
pub mod panic;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::bread_client::BreadClient;

/// Lines buffered for forwarding before new ones are dropped.
const LOG_QUEUE: usize = 512;
/// Most lines in one `bug.logs` request.
const LOG_BATCH: usize = 50;
/// Longest log message forwarded, in bytes.
const MAX_LOG_BYTES: usize = 2048;

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
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: Some(version.into()),
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

/// Best-effort single request: connect, write one line, close. Returns
/// whether it was written. Never blocks longer than the write timeout.
pub(crate) fn send_request(socket: &Path, method: &str, params: Value) -> bool {
    let line = json!({ "id": "0", "method": method, "params": params }).to_string();
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
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

struct Shared {
    app: String,
    socket: PathBuf,
    interval: Duration,
    unconditional: bool,
    /// Milliseconds since `epoch` of the last [`BugGuard::tick`].
    last_tick: AtomicU64,
    epoch: Instant,
    stop: AtomicBool,
    dropped_logs: AtomicU64,
}

/// Keeps the reporting threads alive; dropping it announces a clean exit to
/// the freeze watchdog and stops them.
pub struct BugGuard {
    shared: Arc<Shared>,
    log_tx: Option<SyncSender<Line>>,
    threads: Vec<JoinHandle<()>>,
    heartbeat_on: bool,
}

impl BreadClient {
    /// Turn on crash, freeze and log reporting for this app. Never fails and
    /// never blocks: with breadd absent everything quietly does nothing.
    pub fn enable_bug_reporting(&self, opts: BugOptions) -> BugGuard {
        let socket = opts
            .socket
            .clone()
            .unwrap_or_else(bread_shared::resolve_socket_path);
        let app = self.app_id().to_string();

        if opts.panic_hook {
            panic::install(
                app.clone(),
                opts.version.clone(),
                socket.clone(),
                opts.panic_message,
            );
        }
        if opts.native_crashes {
            native::install(&app, opts.version.as_deref(), &socket);
        }

        let shared = Arc::new(Shared {
            app,
            socket,
            interval: opts.heartbeat_interval.max(Duration::from_millis(250)),
            unconditional: opts.heartbeat_unconditional,
            last_tick: AtomicU64::new(0),
            epoch: Instant::now(),
            stop: AtomicBool::new(false),
            dropped_logs: AtomicU64::new(0),
        });

        let mut threads = Vec::new();
        let mut log_tx = None;
        if opts.forward_logs {
            let (tx, rx) = sync_channel::<Line>(LOG_QUEUE);
            log_tx = Some(tx);
            let sh = shared.clone();
            if let Ok(h) = std::thread::Builder::new()
                .name("bug-logs".into())
                .spawn(move || log_loop(&sh, &rx))
            {
                threads.push(h);
            }
        }
        if opts.heartbeat {
            let sh = shared.clone();
            if let Ok(h) = std::thread::Builder::new()
                .name("bug-heartbeat".into())
                .spawn(move || heartbeat_loop(&sh))
            {
                threads.push(h);
            }
        }
        BugGuard {
            shared,
            log_tx,
            threads,
            heartbeat_on: opts.heartbeat,
        }
    }
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

fn clip(msg: &str) -> String {
    if msg.len() <= MAX_LOG_BYTES {
        return msg.to_string();
    }
    let mut cut = MAX_LOG_BYTES;
    while !msg.is_char_boundary(cut) {
        cut -= 1;
    }
    msg[..cut].to_string()
}

impl BugGuard {
    /// Call from the app's main loop (cheap). Heartbeats are only sent while
    /// ticks keep arriving, so a frozen main loop stops them and breadd can
    /// notice.
    pub fn tick(&self) {
        let ms = self.shared.epoch.elapsed().as_millis() as u64 + 1;
        self.shared.last_tick.store(ms, Ordering::Relaxed);
    }

    /// Tick from the GLib main loop (GTK apps): the timeout only runs while
    /// the UI thread is responsive, which is exactly the signal wanted.
    #[cfg(feature = "gtk")]
    pub fn attach_glib_main_loop(&self) {
        let shared = self.shared.clone();
        let every = (shared.interval / 2).max(Duration::from_millis(100));
        gtk4::glib::timeout_add_local(every, move || {
            let ms = shared.epoch.elapsed().as_millis() as u64 + 1;
            shared.last_tick.store(ms, Ordering::Relaxed);
            gtk4::glib::ControlFlow::Continue
        });
    }

    /// Queue a log line for breadd. Lossy by design: if the queue is full
    /// the line is dropped rather than blocking the caller.
    pub fn log(&self, level: &str, msg: &str) {
        let Some(tx) = &self.log_tx else { return };
        let line = Line {
            level: level_name(level),
            msg: clip(msg),
            t: now_ms(),
        };
        if tx.try_send(line).is_err() {
            self.shared.dropped_logs.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// How many log lines were dropped because the queue was full.
    pub fn dropped_logs(&self) -> u64 {
        self.shared.dropped_logs.load(Ordering::Relaxed)
    }

    /// A `log::Log` backend that forwards records to this guard and nothing
    /// else; chain it with your normal logger if you want both.
    pub fn logger(&self) -> BugLogger {
        BugLogger {
            tx: self.log_tx.clone(),
            shared: self.shared.clone(),
        }
    }
}

impl Drop for BugGuard {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.log_tx = None; // closes the channel; the log thread flushes and exits
        if self.heartbeat_on {
            send_request(
                &self.shared.socket,
                "bug.heartbeat",
                json!({ "app": self.shared.app, "stopping": true }),
            );
        }
        for h in self.threads.drain(..) {
            let _ = h.join();
        }
    }
}

/// `log::Log` adapter; see [`BugGuard::logger`].
pub struct BugLogger {
    tx: Option<SyncSender<Line>>,
    shared: Arc<Shared>,
}

impl log::Log for BugLogger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        self.tx.is_some()
    }

    fn log(&self, record: &log::Record<'_>) {
        let Some(tx) = &self.tx else { return };
        let line = Line {
            level: level_name(record.level().as_str()),
            msg: clip(&format!("{}: {}", record.target(), record.args())),
            t: now_ms(),
        };
        if tx.try_send(line).is_err() {
            self.shared.dropped_logs.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn flush(&self) {}
}

fn log_loop(sh: &Shared, rx: &std::sync::mpsc::Receiver<Line>) {
    let mut backoff_until: Option<Instant> = None;
    loop {
        let mut batch: Vec<Line> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut closed = false;
        while batch.len() < LOG_BATCH {
            let wait = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(l) => batch.push(l),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    closed = true;
                    break;
                }
            }
        }
        if !batch.is_empty() {
            let skip = backoff_until.is_some_and(|t| Instant::now() < t);
            if !skip {
                let lines: Vec<Value> = batch
                    .iter()
                    .map(|l| json!({ "level": l.level, "msg": l.msg, "t": l.t }))
                    .collect();
                let ok = send_request(
                    &sh.socket,
                    "bug.logs",
                    json!({ "app": sh.app, "lines": lines }),
                );
                // With breadd down, stop hammering the socket for a while.
                backoff_until = (!ok).then(|| Instant::now() + Duration::from_secs(5));
            }
        }
        if closed {
            return;
        }
    }
}

fn heartbeat_loop(sh: &Shared) {
    let interval_ms = sh.interval.as_millis() as u64;
    loop {
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
        while Instant::now() < until {
            if sh.stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25).min(sh.interval));
        }
    }
}

#[cfg(test)]
mod tests;
