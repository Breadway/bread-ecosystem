//! Headless app-launcher core — desktop-entry discovery, fuzzy matching and
//! ranking, launch history, and process launching — plus an optional GTK4
//! results-list widget behind the `gtk` feature.
//!
//! Lives in `bread-ecosystem`, not an app repo, so breadbar (which must not
//! depend on an app repo) can embed the same launcher logic breadbox's
//! overlay window already wraps: one implementation, two hosts
//! (`THEME_SYSTEM_PLAN.md` §3, §7).
//!
//! Every path/cache/history entry point here takes an explicit `app: &str`
//! rather than hardcoding an app name, so more than one host can use this
//! crate without colliding — see [`cache_dir`]/[`config_dir`]. [`LAUNCHER_APP`]
//! is the one identity every *launcher* host (as opposed to some unrelated
//! future consumer of `cache_dir`/`config_dir`) should actually pass — see
//! its own doc comment for why.

mod desktop;
mod history;
mod icon;
mod launch;
mod matching;
mod paths;

#[cfg(feature = "gtk")]
pub mod gtk;

pub use desktop::{load_all_desktop_entries, parse_desktop, strip_exec_codes, DesktopEntry};
pub use history::LaunchHistory;
pub use icon::IconCache;
pub use launch::{do_launch, emit_launched};
pub use matching::{fuzzy_matches, fuzzy_score, load_sorted_entries, matches_term, priority_rank};
pub use paths::{app_dirs, cache_dir, config_dir, home_dir};

/// The launcher's one shared identity, passed to [`cache_dir`]/[`config_dir`]/
/// [`IconCache::new`]/[`LaunchHistory::load`] by every host that embeds this
/// crate — breadbox's overlay window AND breadbar's embedded capsule
/// (theme 04/spotlight) alike.
///
/// This is deliberate, not a leftover of breadbox being first: theme 04's
/// whole premise is that breadbar's capsule IS the launcher wearing a
/// different shell, not a second launcher with its own history
/// (`THEME_SYSTEM_PLAN.md` §7). If each host passed its own binary name here,
/// the same physical launcher would rank a user's apps differently
/// depending on which theme happened to be active — the icon cache and
/// "most launched" ordering would silently fork in two. Sharing this
/// constant is what keeps them one launcher.
///
/// Do not pass a bare `"breadbox"` string literal at a call site instead of
/// this constant — that reads exactly like an unfixed bug (breadbar naming
/// another app's identity) and invites a later "fix" that would quietly
/// break the shared history this constant exists to guarantee.
pub const LAUNCHER_APP: &str = "breadbox";
