//! Hyprland IPC client: socket1 request/response (JSON) and socket2 path
//! resolution.
//!
//! The socket-path resolution + raw request/response round trip was
//! duplicated near-verbatim in `breadbox/src/main.rs` (`get_active_workspace`,
//! lines 26-42) and `breadclip/src/position.rs` (`hyprctl_json`, lines
//! 58-71) — same `HYPRLAND_INSTANCE_SIGNATURE`/`XDG_RUNTIME_DIR` env lookup,
//! same `.socket.sock` path format, same connect/write/shutdown-write/
//! read-to-string sequence. `breadmon/src/main.rs`'s `hyprland_socket2_path`
//! duplicates just the path-resolution half for the event socket.
//!
//! `active_window`'s `fullscreen` field deserializes leniently as either a
//! JSON bool or integer: Hyprland has changed this field's type across
//! versions (older releases emit a bool, `0`/`1`; newer ones emit an
//! integer fullscreen *mode* — `0` none, `1` maximized, `2` fullscreen), and
//! a client hard-coded to one shape silently misreads the other instead of
//! erroring. `breadclip`'s own version (`as_i64().unwrap_or(0) != 0`) only
//! handles the integer shape; a bool `true` would `.as_i64()` to `None` and
//! silently read as "not fullscreen".

use serde::Deserialize;
use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

/// Which of Hyprland's two IPC sockets: `.socket.sock` (request/response) or
/// `.socket2.sock` (event stream).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Socket {
    Request,
    Events,
}

/// Resolve the path to one of Hyprland's IPC sockets from
/// `HYPRLAND_INSTANCE_SIGNATURE` + `XDG_RUNTIME_DIR`. Returns `None` if
/// `HYPRLAND_INSTANCE_SIGNATURE` isn't set (Hyprland isn't running, or we're
/// not inside a Hyprland session) — `XDG_RUNTIME_DIR` falls back to
/// `/run/user/1000` if unset, matching `breadmon`'s existing fallback.
pub fn socket_path(kind: Socket) -> Option<PathBuf> {
    let sig = env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let rt = env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string());
    let file = match kind {
        Socket::Request => ".socket.sock",
        Socket::Events => ".socket2.sock",
    };
    Some(PathBuf::from(format!("{rt}/hypr/{sig}/{file}")))
}

/// Send `request` (e.g. `"j/activewindow"`, `"j/monitors"`) to the socket1
/// IPC socket and return the raw response body. Blocking/synchronous — this
/// matches every current consumer (breadbox, breadclip), which call it from
/// non-async GTK app code.
///
/// Read/write timeouts are set on the socket (both original hand-rolled
/// implementations this replaces — breadbox's `get_active_workspace`,
/// breadclip's `hyprctl_json` — had none): a Hyprland instance that's
/// wedged or mid-reload could otherwise hang this call, and every current
/// caller runs it on the GTK main thread, so a hang here freezes the whole
/// UI, not just this query.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

pub fn request(request: &str) -> Option<String> {
    let socket = socket_path(Socket::Request)?;
    let mut stream = UnixStream::connect(&socket).ok()?;
    stream.set_read_timeout(Some(REQUEST_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT)).ok()?;
    stream.write_all(request.as_bytes()).ok()?;
    stream.shutdown(std::net::Shutdown::Write).ok()?;

    let mut buf = String::new();
    stream.read_to_string(&mut buf).ok()?;
    Some(buf)
}

/// Like [`request`], parsed as JSON. `request` should already carry the `j/`
/// prefix Hyprland expects for JSON responses (e.g. `"j/activewindow"`).
pub fn request_json(request_str: &str) -> Option<serde_json::Value> {
    serde_json::from_str(&request(request_str)?).ok()
}

/// Connect to the socket2 event stream. Callers read newline-delimited
/// `EVENT>>DATA` lines from the returned stream themselves — event framing
/// and reconnect/backoff policy are genuinely per-consumer (see
/// `breadmon`'s hotplug listener), so this only replaces the duplicated
/// path-resolution + connect boilerplate, not a full event-loop
/// abstraction.
pub fn connect_events() -> Option<UnixStream> {
    let socket = socket_path(Socket::Events)?;
    UnixStream::connect(&socket).ok()
}

/// Hyprland's `fullscreen` field, tolerant of either representation it has
/// shipped across versions: a plain bool, or an integer fullscreen mode
/// (`0` = none, nonzero = some fullscreen mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FullscreenState(bool);

impl FullscreenState {
    pub fn is_fullscreen(self) -> bool {
        self.0
    }
}

impl<'de> Deserialize<'de> for FullscreenState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Bool(bool),
            Int(i64),
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Bool(b) => FullscreenState(b),
            Repr::Int(i) => FullscreenState(i != 0),
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActiveWindow {
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub fullscreen: FullscreenState,
    pub at: (i32, i32),
    pub size: (i32, i32),
}

impl ActiveWindow {
    pub fn x(&self) -> i32 {
        self.at.0
    }
    pub fn y(&self) -> i32 {
        self.at.1
    }
    pub fn width(&self) -> i32 {
        self.size.0
    }
    pub fn height(&self) -> i32 {
        self.size.1
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Monitor {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default)]
    pub transform: i64,
    #[serde(default)]
    pub focused: bool,
}

fn one() -> f64 {
    1.0
}

impl Monitor {
    /// Logical (scaled, transform-aware) output size, matching what `grim -g`
    /// expects — `width`/`height` above are physical pixels. A 90/270-degree
    /// transform swaps the axes before the scale divide, same as breadshot's
    /// `monitor_geometry`, which this mirrors.
    pub fn logical_size(&self) -> (i32, i32) {
        if self.transform % 2 == 0 {
            (
                (self.width as f64 / self.scale).round() as i32,
                (self.height as f64 / self.scale).round() as i32,
            )
        } else {
            (
                (self.height as f64 / self.scale).round() as i32,
                (self.width as f64 / self.scale).round() as i32,
            )
        }
    }
}

/// One entry from `hyprctl layers -j` — a layer-shell surface (bar, launcher,
/// lock screen, ...), keyed by the `namespace` its client registered.
#[derive(Debug, Clone, Deserialize)]
pub struct Layer {
    pub namespace: String,
    pub pid: u32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Find a layer-shell surface by namespace *and* pid. Namespace alone isn't
/// enough to identify "our own" surface — several bread apps (breadbar
/// notably) are typically already running under the same namespace when a
/// second instance starts up for some other purpose (e.g. screenshot mode),
/// so callers pass their own `std::process::id()` to disambiguate.
pub fn find_layer(namespace: &str, pid: u32) -> Option<Layer> {
    find_layer_in(&request_json("j/layers")?, namespace, pid)
}

/// Parsing half of [`find_layer`], split out so it's testable without a live
/// Hyprland socket.
fn find_layer_in(root: &serde_json::Value, namespace: &str, pid: u32) -> Option<Layer> {
    let monitors = root.as_object()?;
    for monitor in monitors.values() {
        let Some(levels) = monitor.get("levels").and_then(|v| v.as_object()) else {
            continue;
        };
        for layers in levels.values() {
            for layer in layers.as_array().into_iter().flatten() {
                let Ok(l) = serde_json::from_value::<Layer>(layer.clone()) else {
                    continue;
                };
                if l.namespace == namespace && l.pid == pid {
                    return Some(l);
                }
            }
        }
    }
    None
}

/// Query the currently active (focused) window. Returns `None` if the
/// window is fullscreen or no window is focused — same "centre the popup
/// instead" contract `breadclip`'s original `get_active_window` had.
pub fn active_window() -> Option<ActiveWindow> {
    let win: ActiveWindow = serde_json::from_value(request_json("j/activewindow")?).ok()?;
    if win.fullscreen.is_fullscreen() || win.class.is_empty() {
        return None;
    }
    Some(win)
}

/// Query all monitors and return the focused one (or the first, if none
/// report as focused).
pub fn focused_monitor() -> Option<Monitor> {
    let monitors: Vec<Monitor> = serde_json::from_value(request_json("j/monitors")?).ok()?;
    monitors
        .iter()
        .find(|m| m.focused)
        .or_else(|| monitors.first())
        .cloned()
}

/// The active workspace's name (e.g. `"1"`, `"special:scratch"`).
pub fn active_workspace_name() -> Option<String> {
    request_json("j/activeworkspace")?
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_size_divides_by_scale() {
        let m = Monitor {
            name: "eDP-1".into(),
            x: 0,
            y: 0,
            width: 3840,
            height: 2400,
            scale: 2.0,
            transform: 0,
            focused: true,
        };
        assert_eq!(m.logical_size(), (1920, 1200));
    }

    #[test]
    fn logical_size_swaps_axes_on_rotated_transform() {
        let m = Monitor {
            name: "eDP-1".into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1200,
            scale: 1.0,
            transform: 1,
            focused: true,
        };
        assert_eq!(m.logical_size(), (1200, 1920));
    }

    // Real shape confirmed live via `hyprctl layers -j`: per-monitor map ->
    // "levels" map (layer index "0".."3") -> array of layer objects.
    const LAYERS_JSON: &str = r#"{
        "eDP-1": {
            "levels": {
                "0": [
                    {"address":"0x1","x":0,"y":0,"w":1920,"h":1200,"namespace":"awww-daemon","pid":2481}
                ],
                "1": [],
                "2": [
                    {"address":"0x2","x":0,"y":0,"w":1920,"h":32,"namespace":"breadbar","pid":1601316},
                    {"address":"0x3","x":0,"y":0,"w":1920,"h":32,"namespace":"breadbar","pid":9999}
                ],
                "3": []
            }
        }
    }"#;

    #[test]
    fn find_layer_in_matches_namespace_and_pid() {
        let root: serde_json::Value = serde_json::from_str(LAYERS_JSON).unwrap();
        let layer = find_layer_in(&root, "breadbar", 9999).unwrap();
        assert_eq!(layer.pid, 9999);
        assert_eq!(layer.w, 1920);
        assert_eq!(layer.h, 32);
    }

    #[test]
    fn find_layer_in_ignores_same_namespace_wrong_pid() {
        let root: serde_json::Value = serde_json::from_str(LAYERS_JSON).unwrap();
        // pid 1601316 is a real breadbar layer in the fixture, but not the one
        // we're asking for — must not fall back to it.
        assert!(find_layer_in(&root, "breadbar", 42).is_none());
    }

    #[test]
    fn find_layer_in_returns_none_for_missing_namespace() {
        let root: serde_json::Value = serde_json::from_str(LAYERS_JSON).unwrap();
        assert!(find_layer_in(&root, "breadbox", 1601316).is_none());
    }

    #[test]
    fn fullscreen_state_deserializes_from_bool() {
        let s: FullscreenState = serde_json::from_str("true").unwrap();
        assert!(s.is_fullscreen());
        let s: FullscreenState = serde_json::from_str("false").unwrap();
        assert!(!s.is_fullscreen());
    }

    #[test]
    fn fullscreen_state_deserializes_from_int() {
        let s: FullscreenState = serde_json::from_str("0").unwrap();
        assert!(!s.is_fullscreen());
        let s: FullscreenState = serde_json::from_str("2").unwrap();
        assert!(s.is_fullscreen());
    }

    #[test]
    fn active_window_parses_bool_fullscreen_shape() {
        let json = r#"{"class":"kitty","fullscreen":true,"at":[10,20],"size":[300,400]}"#;
        let win: ActiveWindow = serde_json::from_str(json).unwrap();
        assert!(win.fullscreen.is_fullscreen());
        assert_eq!(win.x(), 10);
        assert_eq!(win.height(), 400);
    }

    #[test]
    fn active_window_parses_int_fullscreen_shape() {
        let json = r#"{"class":"kitty","fullscreen":1,"at":[0,0],"size":[100,100]}"#;
        let win: ActiveWindow = serde_json::from_str(json).unwrap();
        assert!(win.fullscreen.is_fullscreen());
    }

    // Both env-var-dependent cases share one test function: `set_var`/
    // `remove_var` are process-global, and cargo runs tests in parallel
    // threads by default, so two separate #[test] fns racing on the same
    // vars would be flaky.
    #[test]
    fn socket_path_env_var_behavior() {
        let _lock = crate::env_test_lock().lock().unwrap_or_else(|e| e.into_inner());
        unsafe { env::remove_var("HYPRLAND_INSTANCE_SIGNATURE") };
        assert!(socket_path(Socket::Request).is_none());

        unsafe {
            env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "test-sig");
            env::set_var("XDG_RUNTIME_DIR", "/run/user/9999");
        }
        assert_eq!(
            socket_path(Socket::Request).unwrap(),
            PathBuf::from("/run/user/9999/hypr/test-sig/.socket.sock")
        );
        assert_eq!(
            socket_path(Socket::Events).unwrap(),
            PathBuf::from("/run/user/9999/hypr/test-sig/.socket2.sock")
        );
        unsafe {
            env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
            env::remove_var("XDG_RUNTIME_DIR");
        }
    }
}
