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

pub mod atomic;
pub mod hypr;
pub mod proc;
pub mod singleton;
pub mod xdg;

#[cfg(feature = "toml")]
pub mod tomlcfg;

#[cfg(feature = "gtk")]
pub mod gtk_popup;
