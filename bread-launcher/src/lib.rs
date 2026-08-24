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
//! crate without colliding — see [`cache_dir`]/[`config_dir`].

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
