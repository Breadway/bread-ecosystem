//! A persistent-connection client for breadd's IPC socket, for sibling
//! `bread*` app daemons that run continuously.
//!
//! This is deliberately a *second* client alongside `bread-emit` (the
//! fire-and-forget CLI binary in the `bread` repo), not a replacement for
//! it: `bread-emit` skips holding a connection open at all, which is right
//! for occasional/hook-style callers (a git hook, a shell prompt) but wrong
//! for a long-running daemon like breadclipd that wants to publish an
//! event on every clipboard change and subscribe to a command stream —
//! reconnecting from scratch for every single emit would be wasteful, and
//! subscribing needs a held-open connection by nature.
//!
//! # Graceful degradation
//!
//! A sibling app must never crash or block because breadd is down,
//! restarting, or was never installed. Concretely:
//! - [`BreadClient::emit`] and [`BreadClient::command`] are best-effort,
//!   fire-and-forget single-shot connections (mirroring `bread-emit`'s
//!   own stance) — if breadd is unreachable, the event is silently
//!   dropped, not an error the caller has to handle.
//! - [`BreadClient::health`] / [`BreadClient::api_version`] return `None`
//!   when breadd is unreachable or the response is missing fields.
//!   Long-running daemons SHOULD log a warning in that case and MUST NOT
//!   crash. [`BreadClient::connect`] never fails just because breadd is
//!   down — do not change that.
//! - [`BreadClient::subscribe`] runs its read loop on a background thread
//!   that reconnects with exponential backoff on any disconnect. The
//!   caller's callback simply stops being invoked while disconnected; it
//!   resumes automatically once breadd comes back.
//!
//! # Namespace enforcement
//!
//! `emit` refuses locally (no network round trip) to publish an event
//! outside the app's own `bread.<app_id>.*` segment, so a misconfigured
//! caller fails fast instead of discovering the mistake from the daemon's
//! rejection. The daemon enforces the same rule server-side regardless.
//!
//! `command` is the outbound half of the same story: it publishes
//! `bread.command.<target_app>.<verb>` as an **unsourced** IPC emit
//! (`params` is `{ event, data }` only — no `source`/`kind`). The
//! daemon treats unsourced `bread.command.<known_app>.*` as legal so a
//! sibling can address another app without impersonating that app's
//! own namespace. Local refusal (eprint + return, same stance as
//! `emit`) if `target_app` or `verb` is empty, or if `verb` contains
//! `.` (a command verb is a single segment).

use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use bread_shared::apps::validate_app_namespace;
use serde_json::{json, Value};

/// A normalized event as delivered by breadd's `events.subscribe` stream.
#[derive(Debug, Clone)]
pub struct BreadEvent {
    /// Dotted event name, e.g. `bread.command.clip.clear`.
    pub event: String,
    /// Unix epoch milliseconds when the daemon observed the originating signal.
    pub timestamp: u64,
    /// Structured event data; shape depends on the event family.
    pub data: Value,
}

/// A client bound to one sibling app's identity, used to `emit` within that
/// app's namespace and `subscribe` to events (typically its own
/// `bread.command.<app_id>.**` verb namespace).
///
/// Cheap to clone (just an `Arc`-free `String`); safe to share across
/// threads by cloning, or to construct fresh per call site.
#[derive(Clone)]
pub struct BreadClient {
    app_id: String,
}

impl BreadClient {
    /// Bind a client to `app_id` (e.g. `"clip"`). Does not connect yet —
    /// there is no persistent connection to "fail" at construction time;
    /// `emit` and `subscribe` each connect (or reconnect) as needed. This
    /// is itself part of the graceful-degradation story: constructing a
    /// `BreadClient` can never fail just because breadd isn't running yet.
    pub fn connect(app_id: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
        }
    }

    /// The app id this client is bound to.
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// Publish `event` (must be within `bread.<app_id>.*`) with `data`.
    /// Fire-and-forget: a single short-lived connection is opened, the
    /// request is written, and the reply is never read (mirroring
    /// `bread-emit`). If breadd is unreachable or slow, this silently does
    /// nothing — it never blocks or errors the caller.
    pub fn emit(&self, event: &str, data: Value) {
        if !validate_app_namespace(&self.app_id, event) {
            eprintln!(
                "bread-client: refusing to emit '{event}' outside the '{}' namespace",
                self.app_id
            );
            return;
        }

        fire_and_forget_emit(json!({
            "event": event,
            "source": self.app_id,
            "kind": event,
            "data": data,
        }));
    }

    /// Publish `bread.command.<target_app>.<verb>` as an unsourced IPC
    /// emit so another bread app (or a Lua workflow) can act on it.
    ///
    /// Fire-and-forget: same silent-if-down stance as [`emit`]. Locally
    /// refuses (eprint + return, no socket) if `target_app` or `verb` is
    /// empty, or if `verb` contains `.` — a verb is one segment
    /// (`clear`, not `history.clear`).
    ///
    /// The wire payload is `{ method: "emit", params: { event, data } }`
    /// with **no** `source`/`kind`. Do not add those: a sourced emit
    /// would have to claim the *target's* namespace (or ours), and the
    /// daemon half of this integration is specifically making unsourced
    /// `bread.command.<known_app>.*` legal.
    pub fn command(&self, target_app: &str, verb: &str, data: Value) {
        if target_app.is_empty() || verb.is_empty() || verb.contains('.') {
            eprintln!(
                "bread-client: refusing to send command to '{target_app}' with verb '{verb}' \
                 (target and verb must be non-empty; verb must be a single segment)"
            );
            return;
        }

        let event = format!("bread.command.{target_app}.{verb}");
        fire_and_forget_emit(json!({
            "event": event,
            "data": data,
        }));
    }

    /// One-shot `health` IPC request. `None` if breadd is unreachable or
    /// the response is malformed / an error.
    ///
    /// Long-running daemons SHOULD log a warning when this returns
    /// `None` (or when [`api_version`] is missing) and MUST NOT crash.
    pub fn health(&self) -> Option<Value> {
        self.request("health", json!({}))
    }

    /// `api_version` string from [`health`], or `None` if health failed
    /// or the field is absent. Same SHOULD-warn / MUST-NOT-crash rule
    /// as [`health`].
    pub fn api_version(&self) -> Option<String> {
        self.health()?
            .get("api_version")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// Send a one-shot IPC request and return its `result`, or `None` on any
    /// failure — breadd unreachable, a malformed response, or an `error`
    /// field in the response. Mirrors `emit`'s graceful-degradation stance:
    /// a caller checks for `None` the same way it'd handle "daemon not
    /// installed," not via a `Result` that forces error-path plumbing for
    /// what is, for most callers (a refresh-on-connect read), an expected
    /// possibility rather than an exceptional one.
    ///
    /// Unlike `emit`, this is not restricted to the client's own namespace —
    /// `method`/`params` map directly onto breadd's IPC method table (see
    /// `Documentation.md`'s "Dictionary: IPC protocol"), most of which
    /// (`state.get`, `widgets.list`, ...) are cross-namespace reads by
    /// design. Only first-party compiled code links `bread-utils`, so this
    /// carries the same trust level as `emit`'s own request construction.
    pub fn request(&self, method: &str, params: Value) -> Option<Value> {
        let request = json!({
            "id": "0",
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&request).ok()?;

        let mut stream = UnixStream::connect(bread_shared::resolve_socket_path()).ok()?;
        stream
            .set_write_timeout(Some(Duration::from_millis(200)))
            .ok()?;
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .ok()?;
        writeln!(stream, "{line}").ok()?;

        let mut response_line = String::new();
        BufReader::new(stream).read_line(&mut response_line).ok()?;
        if response_line.trim().is_empty() {
            return None;
        }

        let value: Value = serde_json::from_str(&response_line).ok()?;
        if value.get("error").is_some() {
            return None;
        }
        value.get("result").cloned()
    }

    /// Subscribe to events matching `pattern` (glob: `*`/`**`/`?`), invoking
    /// `on_event` for each one on a dedicated background thread. Typically
    /// called with `"bread.command.<app_id>.**"` to receive commands
    /// addressed to this app.
    ///
    /// Returns a [`Subscription`] handle; drop or call [`Subscription::stop`]
    /// to end it. The background thread reconnects with exponential backoff
    /// (500ms, capped at ~32s) whenever the connection drops, so a restart
    /// of breadd is transparent to the caller — `on_event` simply pauses
    /// and resumes.
    pub fn subscribe<F>(&self, pattern: impl Into<String>, on_event: F) -> Subscription
    where
        F: Fn(BreadEvent) + Send + 'static,
    {
        let pattern = pattern.into();
        let stop = Arc::new(AtomicBool::new(false));
        let current_stream: Arc<Mutex<Option<UnixStream>>> = Arc::new(Mutex::new(None));

        let stop_for_thread = stop.clone();
        let stream_for_thread = current_stream.clone();
        let handle = thread::spawn(move || {
            let mut attempt: u32 = 0;
            while !stop_for_thread.load(Ordering::Relaxed) {
                match run_subscription_once(&pattern, &on_event, &stream_for_thread) {
                    Ok(()) => attempt = 0, // clean end (stop() closed the socket)
                    Err(_) => attempt = attempt.saturating_add(1),
                }

                *stream_for_thread.lock().unwrap_or_else(|p| p.into_inner()) = None;

                if stop_for_thread.load(Ordering::Relaxed) {
                    break;
                }
                let backoff_ms = 500u64.saturating_mul(2u64.saturating_pow(attempt.min(6)));
                thread::sleep(Duration::from_millis(backoff_ms));
            }
        });

        Subscription {
            stop,
            current_stream,
            handle: Some(handle),
        }
    }
}

/// Fire-and-forget a single `emit` request. Shared by sourced [`BreadClient::emit`]
/// and unsourced [`BreadClient::command`] so the write/timeout path cannot
/// drift. Silent if the socket is missing, the write fails, or the body
/// cannot be serialized.
fn fire_and_forget_emit(params: Value) {
    let request = json!({
        "id": "0",
        "method": "emit",
        "params": params,
    });
    let Ok(line) = serde_json::to_string(&request) else {
        return;
    };

    let Ok(mut stream) = UnixStream::connect(bread_shared::resolve_socket_path()) else {
        return;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
    let _ = writeln!(stream, "{line}");
}

/// Connects once, sends `events.subscribe`, and invokes `on_event` for every
/// matching line until the connection ends (cleanly or with an error).
/// Stores the live stream in `current_stream` so [`Subscription::stop`] can
/// shut it down from another thread to interrupt the blocking read promptly.
fn run_subscription_once(
    pattern: &str,
    on_event: &impl Fn(BreadEvent),
    current_stream: &Mutex<Option<UnixStream>>,
) -> std::io::Result<()> {
    let stream = UnixStream::connect(bread_shared::resolve_socket_path())?;
    let read_stream = stream.try_clone()?;
    *current_stream.lock().unwrap_or_else(|p| p.into_inner()) = Some(stream);

    // Re-borrow to write the subscribe request through the stored copy so
    // there is exactly one owner performing I/O per direction.
    {
        let guard = current_stream.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(stream) = guard.as_ref() {
            let mut writer = stream;
            let request = json!({
                "id": "sub",
                "method": "events.subscribe",
                "params": { "filter": pattern }
            });
            let line = serde_json::to_string(&request).unwrap_or_default();
            writeln!(writer, "{line}")?;
        }
    }

    for line in BufReader::new(read_stream).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // The first line is the subscribe ack ({"result": {"subscribed": true}});
        // only lines with an "event" field are actual BreadEvents.
        if let Some(event_name) = value.get("event").and_then(Value::as_str) {
            let timestamp = value.get("timestamp").and_then(Value::as_u64).unwrap_or(0);
            let data = value.get("data").cloned().unwrap_or(Value::Null);
            on_event(BreadEvent {
                event: event_name.to_string(),
                timestamp,
                data,
            });
        }
    }
    Ok(())
}

/// Handle to a running [`BreadClient::subscribe`] background thread.
pub struct Subscription {
    stop: Arc<AtomicBool>,
    current_stream: Arc<Mutex<Option<UnixStream>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Subscription {
    /// Stop the subscription and block until its background thread exits.
    /// Shuts down the live socket (if connected) so a thread blocked in a
    /// read wakes up immediately, rather than waiting for the next event or
    /// a future reconnect attempt to notice the stop flag.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(stream) = self
            .current_stream
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(stream) = self
            .current_stream
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
        // Best-effort on drop: don't block a caller who simply let the
        // handle go out of scope. Explicit `stop()` is what actually waits.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_never_fails_even_with_no_daemon_present() {
        // Constructing a client must not depend on breadd actually running —
        // that's the whole point of the graceful-degradation design.
        let _client = BreadClient::connect("clip");
    }

    #[test]
    fn emit_is_a_silent_no_op_when_daemon_is_unreachable() {
        // Point at a socket path that can't possibly exist by using an
        // app id that still passes namespace validation; the daemon being
        // absent must not panic or block this call.
        let client = BreadClient::connect("clip");
        client.emit("bread.clip.copied", json!({ "len": 1 }));
    }

    #[test]
    fn emit_refuses_event_outside_own_namespace_without_connecting() {
        // "pad" events are not this client's to publish — this must be
        // caught locally (and cheaply) rather than round-tripped to a
        // daemon that isn't even running in this test.
        let client = BreadClient::connect("clip");
        client.emit("bread.pad.reminder.due", json!({}));
        // No assertion beyond "did not panic" — there is no daemon to
        // observe the (correctly suppressed) call against in a unit test;
        // the cross-process behavior is covered by breadd's own
        // integration tests for the IPC-side of namespace validation.
    }

    /// Point `HOME` + `XDG_RUNTIME_DIR` at an empty temp dir so
    /// `resolve_socket_path` cannot find a live breadd (either via
    /// `~/.config/bread/breadd.toml` or `$XDG_RUNTIME_DIR/bread/breadd.sock`).
    /// Serialized with the other env-mutating tests via `env_test_lock`.
    fn with_unreachable_daemon<T>(f: impl FnOnce() -> T) -> T {
        let _lock = crate::env_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let old_home = std::env::var("HOME").ok();
        let old_xdg = std::env::var("XDG_RUNTIME_DIR").ok();
        unsafe {
            std::env::set_var("HOME", tmp.path());
            std::env::set_var("XDG_RUNTIME_DIR", tmp.path());
        }
        struct Restore(Option<String>, Option<String>);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe {
                    match &self.0 {
                        Some(v) => std::env::set_var("HOME", v),
                        None => std::env::remove_var("HOME"),
                    }
                    match &self.1 {
                        Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                        None => std::env::remove_var("XDG_RUNTIME_DIR"),
                    }
                }
            }
        }
        let _restore = Restore(old_home, old_xdg);
        f()
    }

    #[test]
    fn request_returns_none_when_daemon_is_unreachable() {
        with_unreachable_daemon(|| {
            let client = BreadClient::connect("clip");
            assert!(client.request("widgets.list", json!(null)).is_none());
        });
    }

    #[test]
    fn command_refuses_empty_target_without_connecting() {
        let client = BreadClient::connect("clip");
        client.command("", "clear", json!({}));
    }

    #[test]
    fn command_refuses_empty_verb_without_connecting() {
        let client = BreadClient::connect("clip");
        client.command("clip", "", json!({}));
    }

    #[test]
    fn command_refuses_dotted_verb_without_connecting() {
        // A verb is a single segment — `history.clear` would produce
        // `bread.command.clip.history.clear`, which is two verb segments.
        let client = BreadClient::connect("clip");
        client.command("clip", "history.clear", json!({}));
    }

    #[test]
    fn command_is_a_silent_no_op_when_daemon_is_unreachable() {
        let client = BreadClient::connect("clip");
        client.command("clip", "clear", json!({ "n": 1 }));
    }

    #[test]
    fn health_and_api_version_return_none_when_daemon_is_unreachable() {
        with_unreachable_daemon(|| {
            let client = BreadClient::connect("clip");
            assert!(client.health().is_none());
            assert!(client.api_version().is_none());
        });
    }

    #[test]
    fn subscription_stop_joins_the_background_thread() {
        let client = BreadClient::connect("clip");
        let sub = client.subscribe("bread.command.clip.**", |_event| {});
        // Even with no daemon present (so the thread is spinning on
        // connect-refused + backoff), stop() must return promptly rather
        // than hanging.
        sub.stop();
    }
}
