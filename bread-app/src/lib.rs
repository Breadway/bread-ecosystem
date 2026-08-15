//! GTK application bootstrap for bread desktop tools.
//!
//! New GTK tools should depend on this crate instead of copying a sixth
//! `main.rs` that wires a `com.breadway.*` application id, a
//! [`bread_utils::singleton`] lock, a layer-shell overlay, and a
//! `bread.command.<app>.**` listen loop.
//!
//! # What this is
//!
//! The pieces every bread GTK binary already copies:
//!
//! - [`application_id`] / [`parse_app_name`] — reverse-DNS id
//!   (`com.breadway.breadbox`) and the same name used for the singleton
//!   pid file.
//! - [`try_acquire`] / [`toggle_or_kill`] — [`bread_utils::singleton`]
//!   wrappers that reject an invalid name before touching the lock.
//! - feature `gtk` — re-exports [`gtk_popup`] (`bread_utils::gtk_popup`)
//!   for the full-screen overlay breadbox / breadclip / breadcast start
//!   from.
//! - feature `bread-client` — [`listen_commands`] plus [`command_verb`] /
//!   [`command_pattern`] so a tool can honor `bread.command.<app>.**`
//!   without re-deriving the prefix strip.
//!
//! This crate does **not** migrate existing apps. Callers still own their
//! widgets, CSS, and clap. Screenshot / `--screenshot` helpers stay in
//! [`bread_utils::screenshot_cli`].
//!
//! # Example
//!
//! ```ignore
//! let _guard = match bread_app::try_acquire("breadbox")? {
//!     bread_app::singleton::Acquire::Acquired(g) => g,
//!     bread_app::singleton::Acquire::HeldByOther(_) => return Ok(()),
//! };
//! let app = gtk4::Application::builder()
//!     .application_id(&bread_app::application_id("breadbox")?)
//!     .build();
//!
//! #[cfg(feature = "gtk")]
//! app.connect_activate(|app| {
//!     let window = bread_app::gtk_popup::new_overlay_window(app, "breadbox");
//!     window.present();
//! });
//!
//! #[cfg(feature = "bread-client")]
//! let _commands = bread_app::listen_commands("box", |verb, event| {
//!     // verb is the single segment after `bread.command.box.`
//!     let _ = (verb, event);
//! })?;
//! ```

pub use bread_utils::singleton;

#[cfg(feature = "gtk")]
pub use bread_utils::gtk_popup;

mod id;

pub use id::{application_id, parse_app_name, toggle_or_kill, try_acquire, InvalidAppId};

#[cfg(feature = "bread-client")]
mod command;

#[cfg(feature = "bread-client")]
pub use bread_utils::bread_client::{BreadClient, BreadEvent, Subscription};
#[cfg(feature = "bread-client")]
pub use command::{command_pattern, command_verb, listen_commands, parse_command_id};
