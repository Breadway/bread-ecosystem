//! Shared plumbing for the bread desktop-automation ecosystem.
//!
//! Extracted from genuine, verified duplication across breadbox, breadclip,
//! breadmon, breadcrumbs, bos-settings, and breadhelp during the 2026-07-16
//! ecosystem-wide utility audit. Each module's doc comment cites the
//! original file:line locations the code was extracted from.
//!
//! - [`hypr`] — Hyprland IPC: socket path resolution, socket1
//!   request/response, typed `activewindow`/`monitors` queries with
//!   version-tolerant `fullscreen` field parsing.
//! - [`singleton`] — correct, TOCTOU-free single-instance/PID-toggle.
//! - [`proc`] — timeout-guarded subprocess execution.
//! - [`atomic`] — atomic (temp-then-rename) file writes, with an optional
//!   `.bak`-before-overwrite variant.
//! - [`xdg`] — XDG base directory helpers with a real (never literal-tilde)
//!   `$HOME` fallback.
//! - [`tomlcfg`] (feature `toml`) — non-destructive TOML document
//!   load/save discipline built on [`atomic`].
//! - [`gtk_popup`] (feature `gtk`) — shared layer-shell popup window setup,
//!   list navigation, and click-outside-to-close.
//! - [`bread_client`] (feature `bread-client`) — a persistent-connection
//!   client for breadd's IPC socket (emit + subscribe), for sibling
//!   `bread*` app daemons integrating with the bread automation fabric.

pub mod atomic;
pub mod hypr;
pub mod proc;
pub mod singleton;
pub mod xdg;

/// Serializes tests that read or mutate process-global env vars
/// (`XDG_RUNTIME_DIR`, `HYPRLAND_INSTANCE_SIGNATURE`) — `cargo test` runs
/// tests in parallel threads within one process by default, and
/// `std::env::set_var` is process-wide, so a `hypr` test temporarily
/// pointing `XDG_RUNTIME_DIR` at a nonexistent path can otherwise race a
/// concurrently-running `singleton` or `xdg` test that expects the real one.
#[cfg(test)]
pub(crate) fn env_test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(feature = "toml")]
pub mod tomlcfg;

#[cfg(feature = "gtk")]
pub mod gtk_popup;

#[cfg(feature = "bread-client")]
pub mod bread_client;
