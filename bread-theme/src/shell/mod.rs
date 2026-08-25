//! `bread_theme::shell` — the shell theme manifest system (Phase 1 of the
//! `THEME_SYSTEM_PLAN.md` design: manifest types, discovery, `extends`
//! merge, validation, `css()`, `watch()`, and the one compiled-in
//! `liquid-motion` builtin describing breadbar/breadbox as they exist
//! today).
//!
//! This module is intentionally gtk-free except for [`watch`] (only
//! compiled under the `gtk` feature, since `gio::FileMonitor` is a gtk4
//! dependency) — `bread` (the daemon) and `breadcrumbs` (the CLI) can read
//! and validate a theme without linking GTK at all.
//!
//! ## Discovery (plan §4)
//!
//! A theme id resolves through, first hit wins:
//! 1. `$XDG_CONFIG_HOME/bread/themes/<id>/theme.toml` (user)
//! 2. `/usr/share/bread/themes/<id>/theme.toml` (system, BOS package)
//! 3. the compiled-in builtin (currently only `liquid-motion`)
//!
//! The *active* id comes from `~/.config/bread/shell.toml`'s `active = "..."`
//! key, overridden by `$BREAD_SHELL_THEME` (the `--theme` CLI flag mentioned
//! in the plan is a consumer-side concern — breadbar/breadbox would set
//! `$BREAD_SHELL_THEME` themselves before calling [`load`], rather than this
//! crate parsing argv).
//!
//! ## `extends` (plan §4/§11)
//!
//! One level, deep-merged: a theme's raw TOML is merged over its `extends`
//! target's raw TOML (child wins key-by-key, recursing into tables; arrays —
//! including slot lists — are replaced wholesale, not concatenated). If the
//! base itself declares `extends`, that second-level `extends` is dropped
//! before merging — chains longer than one level are not supported, by
//! design (plan explicitly scopes this to "one level").
//!
//! ## Never fails (plan §4/§5)
//!
//! [`load`] cannot fail: a missing or malformed *active* theme falls back to
//! the compiled-in builtin, logging once via `tracing::warn!`
//! ([`load_named`] is the fallible primitive underneath, for callers that
//! want to know *why* — e.g. a "broken theme" banner in bos-settings).

mod builtin;
mod manifest;
mod types;

#[cfg(feature = "gtk")]
mod hotreload;
#[cfg(feature = "gtk")]
pub use hotreload::watch;

pub use types::*;

use anyhow::{anyhow, Context};
use std::collections::BTreeMap;
use std::path::PathBuf;

use manifest::RawManifest;

/// A theme, fully resolved: every default filled, every enum validated.
/// Built once by [`load`]/[`load_named`] and handed to consumers as an
/// immutable snapshot — a theme *change* (edit, `extends` retarget, hot
/// reload) produces a new `ShellTheme` rather than mutating this one.
#[derive(Debug, Clone, PartialEq)]
pub struct ShellTheme {
    name: String,
    id: String,
    tokens: Tokens,
    window: WindowSpec,
    slots: Slots,
    modules: Modules,
    launcher: Launcher,
    surfaces: BTreeMap<String, Surface>,
    compositor: BTreeMap<String, LayerRule>,
    css_template: String,
    extra_css: Option<String>,
}

impl ShellTheme {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn tokens(&self) -> &Tokens {
        &self.tokens
    }
    pub fn window(&self) -> &WindowSpec {
        &self.window
    }
    pub fn slots(&self) -> &Slots {
        &self.slots
    }
    pub fn modules(&self) -> &Modules {
        &self.modules
    }
    pub fn launcher(&self) -> &Launcher {
        &self.launcher
    }
    pub fn surfaces(&self) -> &BTreeMap<String, Surface> {
        &self.surfaces
    }
    pub fn compositor_rules(&self) -> &BTreeMap<String, LayerRule> {
        &self.compositor
    }

    /// Token substitution into the theme's CSS template, plus the optional
    /// `extra.css` overlay appended last — plan §5.
    ///
    /// `palette` is accepted to match the plan §5 signature and for parity
    /// with [`crate::stylesheet_resolved`]-style consumers in the future,
    /// but is deliberately unused today: per this task's brief, `@accent` /
    /// `@on-bg` *pass through untouched* here, the same way
    /// [`crate::stylesheet`] leaves them for GTK's own `@define-color`
    /// mechanism to resolve once the CSS provider is attached
    /// (`bgtk::bind_window_with_app_css` / `apply_app_css`). A caller that
    /// wants hex-resolved CSS combines this with
    /// [`crate::resolve_color_names`] itself, exactly as
    /// `breadbar::theme::load_css_for` does today.
    #[allow(unused_variables)]
    pub fn css(&self, palette: &crate::Palette) -> String {
        let mut out = self.tokens.substitute(&self.css_template);
        if let Some(extra) = &self.extra_css {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&self.tokens.substitute(extra));
        }
        out
    }
}

/// Where a discovered theme's manifest text came from — also [`ThemeSummary`]'s
/// `source` field for a picker UI (bos-settings, plan §5 `list()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeSource {
    User,
    System,
    Builtin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeSummary {
    pub id: String,
    pub name: String,
    pub source: ThemeSource,
}

fn config_home() -> PathBuf {
    if let Ok(v) = std::env::var("XDG_CONFIG_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
}

fn user_themes_dir() -> PathBuf {
    config_home().join("bread/themes")
}

fn system_themes_dir() -> PathBuf {
    PathBuf::from("/usr/share/bread/themes")
}

fn user_theme_path(id: &str) -> PathBuf {
    user_themes_dir().join(id).join("theme.toml")
}

fn system_theme_path(id: &str) -> PathBuf {
    system_themes_dir().join(id).join("theme.toml")
}

enum Source {
    User(PathBuf),
    System(PathBuf),
    /// Carries the id rather than being a bare unit variant now that there's
    /// more than one compiled-in theme — [`read_source`]/[`css_template_for`]
    /// need to know *which* builtin's assets to hand back.
    Builtin(String),
}

fn find_source(id: &str) -> Option<Source> {
    let user = user_theme_path(id);
    if user.is_file() {
        return Some(Source::User(user));
    }
    let system = system_theme_path(id);
    if system.is_file() {
        return Some(Source::System(system));
    }
    if builtin::find(id).is_some() {
        return Some(Source::Builtin(id.to_string()));
    }
    None
}

/// Manifest text plus, for on-disk sources, the directory it lives in (used
/// to resolve a relative `css = "extra.css"` overlay path).
fn read_source(src: &Source) -> anyhow::Result<(String, Option<PathBuf>)> {
    match src {
        Source::User(p) | Source::System(p) => {
            let text =
                std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
            Ok((text, p.parent().map(|d| d.to_path_buf())))
        }
        Source::Builtin(id) => {
            let toml = builtin::find(id)
                .map(|t| t.toml.to_string())
                .unwrap_or_default();
            Ok((toml, None))
        }
    }
}

fn css_template_for(id: &str) -> String {
    builtin::find(id)
        .map(|t| t.css.to_string())
        .unwrap_or_default()
}

/// The fallible primitive: look up `id` through discovery, apply one level
/// of `extends`, validate, and resolve. Returns `Err` (naming the offending
/// key/path) rather than falling back — [`load`] is the caller that turns a
/// failure into the builtin.
pub fn load_named(id: &str) -> anyhow::Result<ShellTheme> {
    resolve_theme(id, 0)
}

fn resolve_theme(id: &str, extends_depth: u8) -> anyhow::Result<ShellTheme> {
    let src = find_source(id).ok_or_else(|| {
        anyhow!("no theme named '{id}' (checked user config, system dir, and builtins)")
    })?;
    let (text, dir) = read_source(&src)?;
    let mut value: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing theme '{id}'"))?;

    let extends = value
        .get("extends")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let mut css_template = css_template_for(id);

    if let (Some(base_id), 0) = (&extends, extends_depth) {
        let base_src = find_source(base_id)
            .ok_or_else(|| anyhow!("theme '{id}' extends unknown theme '{base_id}'"))?;
        let (base_text, _base_dir) = read_source(&base_src)?;
        let mut base_value: toml::Value = toml::from_str(&base_text)
            .with_context(|| format!("parsing base theme '{base_id}' (extended by '{id}')"))?;
        // Cap at one level: drop the base's own `extends` so a chain can't
        // go deeper (plan §4: "one level, deep-merged").
        if let toml::Value::Table(t) = &mut base_value {
            t.remove("extends");
        }
        css_template = css_template_for(base_id);
        value = manifest::merge_values(base_value, value);
    }

    let raw: RawManifest = value
        .try_into()
        .with_context(|| format!("theme '{id}' has an invalid or unrecognized field"))?;

    manifest::validate_slots(&raw, id)?;

    let extra_css = match &raw.css {
        Some(rel) => {
            let dir = dir.ok_or_else(|| {
                anyhow!(
                    "theme '{id}' sets css = \"{rel}\" but has no on-disk directory to \
                     resolve it against"
                )
            })?;
            let path = dir.join(rel);
            Some(
                std::fs::read_to_string(&path)
                    .with_context(|| format!("reading css overlay {}", path.display()))?,
            )
        }
        None => None,
    };

    raw.resolve(id, css_template, extra_css)
}

/// Bypasses discovery entirely and resolves straight from the compiled-in
/// `liquid-motion` assets — used as [`load`]'s fallback specifically
/// *because* it cannot be affected by a broken user override file at the
/// same id (unlike calling `load_named("liquid-motion")` again, which would
/// hit that same broken file first via discovery and fail identically).
/// Deliberately always `liquid-motion`, not "whichever theme was active" or
/// "the first entry in `builtin::ALL`" — this is the one theme every other
/// fallback path in this module bottoms out at, so its identity has to stay
/// pinned regardless of how many more builtins `glass-workbench` grows
/// siblings.
fn resolve_builtin() -> ShellTheme {
    let asset = builtin::find(builtin::LIQUID_MOTION_ID)
        .expect("liquid-motion must always be present in builtin::ALL");
    let value: toml::Value =
        toml::from_str(asset.toml).expect("compiled-in builtin theme.toml must parse");
    let raw: RawManifest = value
        .try_into()
        .expect("compiled-in builtin theme.toml must satisfy the manifest schema");
    manifest::validate_slots(&raw, builtin::LIQUID_MOTION_ID)
        .expect("compiled-in builtin theme.toml must use only known module names");
    raw.resolve(builtin::LIQUID_MOTION_ID, asset.css.to_string(), None)
        .expect("compiled-in builtin theme.toml must resolve")
}

static FALLBACK_LOGGED: std::sync::Once = std::sync::Once::new();

/// The active theme. Never fails: a missing or malformed active theme logs
/// once (`tracing::warn!`) and falls back to the compiled-in builtin — "the
/// shell must never fail to start because a theme file is malformed" (plan
/// §4).
pub fn load() -> ShellTheme {
    let id = active_theme_id();
    match load_named(&id) {
        Ok(theme) => theme,
        Err(err) => {
            FALLBACK_LOGGED.call_once(|| {
                tracing::warn!(
                    "bread-theme: shell theme '{id}' failed to load ({err:#}); \
                     falling back to builtin '{}'",
                    builtin::LIQUID_MOTION_ID
                );
            });
            resolve_builtin()
        }
    }
}

fn active_theme_id() -> String {
    if let Ok(v) = std::env::var("BREAD_SHELL_THEME") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    let path = config_home().join("bread/shell.toml");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(value) = toml::from_str::<toml::Value>(&text) {
            if let Some(active) = value.get("active").and_then(|v| v.as_str()) {
                if !active.trim().is_empty() {
                    return active.to_string();
                }
            }
        }
    }
    builtin::LIQUID_MOTION_ID.to_string()
}

fn scan_theme_dir(
    dir: &std::path::Path,
    source: ThemeSource,
    out: &mut Vec<ThemeSummary>,
    seen: &mut std::collections::HashSet<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let toml_path = entry.path().join("theme.toml");
        let Ok(text) = std::fs::read_to_string(&toml_path) else {
            continue;
        };
        let Ok(value) = toml::from_str::<toml::Value>(&text) else {
            continue;
        };
        let id = value
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| entry.file_name().to_str().map(|s| s.to_string()));
        let Some(id) = id else { continue };
        if !seen.insert(id.clone()) {
            continue;
        }
        let name = value
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&id)
            .to_string();
        out.push(ThemeSummary { id, name, source });
    }
}

/// Every theme discoverable across user config, system dir, and builtins —
/// for a picker UI (plan §5: "bos-settings picker"). User shadows system
/// shadows builtin for the same id, matching [`load_named`]'s discovery
/// order.
pub fn list() -> Vec<ThemeSummary> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    scan_theme_dir(&user_themes_dir(), ThemeSource::User, &mut out, &mut seen);
    scan_theme_dir(
        &system_themes_dir(),
        ThemeSource::System,
        &mut out,
        &mut seen,
    );
    for b in builtin::ALL {
        if seen.insert(b.id.to_string()) {
            out.push(ThemeSummary {
                id: b.id.to_string(),
                name: b.name.to_string(),
                source: ThemeSource::Builtin,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        dir: PathBuf,
        old_xdg: Option<String>,
        old_theme_var: Option<String>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match &self.old_theme_var {
                Some(v) => std::env::set_var("BREAD_SHELL_THEME", v),
                None => std::env::remove_var("BREAD_SHELL_THEME"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Isolated `$XDG_CONFIG_HOME` pointing at a fresh temp dir, with
    /// `BREAD_SHELL_THEME` cleared so `active_theme_id()` can't pick up
    /// whatever's set in the outer test-runner environment. Held for the
    /// guard's lifetime.
    fn isolated_xdg() -> EnvGuard {
        // Shared with `layerrules::tests`, which also isolates
        // XDG_CONFIG_HOME — see `crate::test_support` for why this must be
        // the *same* lock rather than a module-private one.
        let lock = crate::test_support::XDG_CONFIG_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "bread-theme-shell-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let old_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let old_theme_var = std::env::var("BREAD_SHELL_THEME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        std::env::remove_var("BREAD_SHELL_THEME");
        EnvGuard {
            _lock: lock,
            dir,
            old_xdg,
            old_theme_var,
        }
    }

    fn write_theme(xdg: &EnvGuard, id: &str, toml_body: &str) {
        let dir = xdg.dir.join("bread/themes").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("theme.toml"), toml_body).unwrap();
    }

    // ---- builtin fidelity -------------------------------------------------

    #[test]
    fn builtin_window_spec_matches_current_breadbar_constants() {
        // breadbar/src/main.rs:16-22: BAR_HEIGHT=44, BAR_MARGIN_TOP=12,
        // BAR_MARGIN_SIDES=16, CHIP_HEIGHT=32, ICON_PX=24. The demo's
        // 14px side margin is deliberately NOT what this asserts.
        let theme = resolve_builtin();
        let w = theme.window();
        assert_eq!(w.height, 44);
        assert_eq!(w.margin.top, 12);
        assert_eq!(w.margin.left, 16);
        assert_eq!(w.margin.right, 16);
        assert_eq!(w.anchors, vec!["top", "left", "right"]);
        assert!(matches!(w.width, Width::Fill));
        assert!(matches!(w.exclusive, Exclusive::Auto));
        assert!(matches!(w.keyboard, Keyboard::None));
        assert_eq!(w.layer, "top");

        assert_eq!(theme.tokens().chip_height(), 32);
        assert_eq!(theme.tokens().icon_px(), 24);
    }

    #[test]
    fn builtin_tokens_match_theme_rs_load_css_locals() {
        // theme.rs::load_css: radius="12px", radius_bar="16px",
        // radius_sm="9px", radius_pill="999px", pad="12px".
        let theme = resolve_builtin();
        let t = theme.tokens();
        assert_eq!(t.radius_card(), 12);
        assert_eq!(t.radius_bar(), 16);
        assert_eq!(t.radius_sm(), 9);
        assert_eq!(t.radius_pill(), 999);
        assert_eq!(t.pad(), 12);
        assert_eq!(t.spring(), "cubic-bezier(0.22, 1.35, 0.36, 1)");
        assert_eq!(t.spring_settle(), "cubic-bezier(0.22, 1.2, 0.36, 1)");
    }

    #[test]
    fn builtin_covers_all_five_breadbar_namespaces_plus_breadbox() {
        let theme = resolve_builtin();
        let rules = theme.compositor_rules();
        for ns in [
            "breadbar",
            "breadbar-osd",
            "breadbar-notif",
            "breadbar-panel",
            "breadbar-dismiss",
            "breadbox",
        ] {
            assert!(rules.contains_key(ns), "missing compositor rule for {ns}");
        }
        assert!(rules["breadbar"].blur);
        assert!(rules["breadbar"].blur_popups);
        assert_eq!(rules["breadbar"].animation.as_deref(), Some("slide top"));
        assert!(rules["breadbar-dismiss"].no_anim);
        assert!(!rules["breadbar-dismiss"].blur);
    }

    #[test]
    fn builtin_surfaces_are_keyed_by_namespace_and_cover_all_four() {
        let theme = resolve_builtin();
        let surfaces = theme.surfaces();
        for ns in [
            "breadbar-notif",
            "breadbar-osd",
            "breadbar-panel",
            "breadbar-dismiss",
        ] {
            assert!(surfaces.contains_key(ns), "missing surface spec for {ns}");
        }
        assert_eq!(surfaces["breadbar-osd"].anchor, "bottom_centre");
        assert!(matches!(
            surfaces["breadbar-panel"].width,
            SurfaceWidth::Auto
        ));
        assert!(matches!(
            surfaces["breadbar-dismiss"].width,
            SurfaceWidth::Fill
        ));
    }

    #[test]
    fn builtin_slots_and_modules_are_trail_and_flip() {
        let theme = resolve_builtin();
        assert_eq!(
            theme.slots().left,
            vec!["workspaces", "widget:right_of_workspaces"]
        );
        assert_eq!(
            theme.slots().centre,
            vec!["media", "widget:left_of_clock", "clock", "widget:right_of_clock"]
        );
        assert!(matches!(
            theme.modules().workspaces.style,
            WorkspaceStyle::Trail
        ));
        assert!(matches!(theme.modules().clock.style, ClockStyle::Flip));
    }

    #[test]
    fn builtin_launcher_matches_current_breadbox_geometry() {
        // breadbox/breadbox/src/main.rs:341-346 (margin/size), :151
        // (build_css's .launcher-bg radius), :174-193 (make_icon's
        // set_pixel_size calls). radius=8 and icon_px=32 are current CODE
        // values, not the demo's 20/36 — see Phase 4b-i's manifest audit.
        let theme = resolve_builtin();
        let l = theme.launcher();
        assert!(matches!(l.mode, LauncherMode::Overlay));
        assert_eq!(l.width, 600);
        assert_eq!(l.top, "120px");
        assert_eq!(l.radius, 8);
        assert_eq!(l.icon_px, 32);
    }

    #[test]
    fn builtin_css_substitutes_tokens_and_leaves_palette_names_untouched() {
        let theme = resolve_builtin();
        let css = theme.css(&crate::Palette::default());
        assert!(
            css.contains("border-radius: 16px"),
            "radius_bar not substituted:\n{css}"
        );
        assert!(
            css.contains("cubic-bezier(0.22, 1.35, 0.36, 1)"),
            "spring not substituted:\n{css}"
        );
        assert!(
            css.contains("@accent, @teal"),
            "accent_from/accent_to gradient not substituted:\n{css}"
        );
        assert!(
            css.contains("@on-bg"),
            "palette name resolved when it should pass through:\n{css}"
        );
        for placeholder in [
            "{radius_bar}",
            "{radius_card}",
            "{radius_sm}",
            "{radius_pill}",
            "{pad}",
            "{spring}",
            "{spring_settle}",
            "{bg_alpha}",
            "{accent_from}",
            "{accent_to}",
            "{chip_height}",
        ] {
            assert!(
                !css.contains(placeholder),
                "unsubstituted token placeholder {placeholder}:\n{css}"
            );
        }
    }

    // ---- glass-workbench builtin (plan §11 phase 5) -----------------------

    #[test]
    fn glass_workbench_loads_and_appears_in_list() {
        let theme = load_named(builtin::GLASS_WORKBENCH_ID)
            .expect("glass-workbench builtin should resolve");
        assert_eq!(theme.id(), "glass-workbench");
        assert_eq!(theme.name(), "Glass Workbench");

        let summaries = list();
        assert!(
            summaries
                .iter()
                .any(|s| s.id == "glass-workbench" && s.source == ThemeSource::Builtin),
            "glass-workbench missing from list(): {summaries:?}"
        );
        // Both builtins must be listed side by side — Phase 5 must not have
        // dropped liquid-motion in the process of adding a second theme.
        assert!(summaries
            .iter()
            .any(|s| s.id == "liquid-motion" && s.source == ThemeSource::Builtin));
    }

    #[test]
    fn glass_workbench_window_is_flush_edge_to_edge_not_a_floating_island() {
        let theme =
            load_named(builtin::GLASS_WORKBENCH_ID).expect("glass-workbench should resolve");
        let w = theme.window();
        assert_eq!(w.anchors, vec!["top", "left", "right"]);
        assert!(matches!(w.width, Width::Fill));
        assert_eq!(w.height, 36);
        assert_eq!(
            w.margin,
            Margin {
                top: 0,
                left: 0,
                right: 0,
                bottom: 0
            },
            "flush bar must have no margin on any edge"
        );
        assert_eq!(
            theme.tokens().radius_bar(),
            0,
            "flush edge-to-edge bar must have square corners"
        );
        assert_eq!(
            theme.tokens().bar_border(),
            "bottom",
            "flush bar draws only a bottom hairline, not liquid-motion's full border"
        );
    }

    #[test]
    fn glass_workbench_modules_are_pill_and_plain_with_no_media_slot() {
        let theme =
            load_named(builtin::GLASS_WORKBENCH_ID).expect("glass-workbench should resolve");
        assert!(matches!(
            theme.modules().workspaces.style,
            WorkspaceStyle::Pill
        ));
        assert!(matches!(theme.modules().clock.style, ClockStyle::Plain));
        assert!(theme.modules().clock.show_date);
        assert_eq!(theme.slots().centre, vec!["clock"]);
        assert!(
            !theme.slots().centre.contains(&"media".to_string()),
            "demo 02 has no media widget"
        );
        assert_eq!(
            theme.slots().right,
            vec!["cpu", "ram", "wifi", "battery", "control"]
        );
    }

    #[test]
    fn glass_workbench_accent_is_flat_and_a_palette_token_not_hex() {
        let theme =
            load_named(builtin::GLASS_WORKBENCH_ID).expect("glass-workbench should resolve");
        let t = theme.tokens();
        assert_eq!(t.accent_from(), "green");
        assert_eq!(t.accent_to(), "green");
        let css = theme.css(&crate::Palette::default());
        assert!(
            css.contains("@green"),
            "accent_from/accent_to must resolve to the @green palette token, not a hex literal:\n{css}"
        );
        assert!(
            !css.contains("#7a9a88"),
            "the demo's sage hex must never leak into the manifest — pywal theming depends on \
             this staying a palette token name:\n{css}"
        );
    }

    // ---- spotlight builtin (plan §11 phase 6) ------------------------------

    #[test]
    fn spotlight_loads_and_appears_in_list_alongside_the_other_two() {
        let theme = load_named(builtin::SPOTLIGHT_ID).expect("spotlight builtin should resolve");
        assert_eq!(theme.id(), "spotlight");
        assert_eq!(theme.name(), "Spotlight");

        let summaries = list();
        assert!(
            summaries
                .iter()
                .any(|s| s.id == "spotlight" && s.source == ThemeSource::Builtin),
            "spotlight missing from list(): {summaries:?}"
        );
        // All three builtins must be listed side by side — adding the third
        // must not have displaced either of the first two.
        for id in ["liquid-motion", "glass-workbench"] {
            assert!(
                summaries
                    .iter()
                    .any(|s| s.id == id && s.source == ThemeSource::Builtin),
                "{id} missing from list() after adding spotlight: {summaries:?}"
            );
        }
    }

    #[test]
    fn spotlight_window_is_a_centred_capsule_not_a_full_width_bar() {
        let theme = load_named(builtin::SPOTLIGHT_ID).expect("spotlight should resolve");
        let w = theme.window();
        // Anchored top ONLY — neither left nor right, which is what makes
        // gtk4-layer-shell centre the surface instead of stretching it
        // (THEME_SYSTEM_PLAN.md §7).
        assert_eq!(w.anchors, vec!["top"]);
        assert_eq!(w.width, Width::Px(480));
        assert_eq!(w.height, 36);
        assert_eq!(w.margin.top, 16);
        assert_eq!(
            w.exclusive,
            Exclusive::None,
            "the capsule floats over tiled content, unlike Island/Edge's reserved strip"
        );
        assert_eq!(
            w.keyboard,
            Keyboard::OnDemand,
            "keyboard focus hands over only while launcher_entry holds it"
        );
    }

    #[test]
    fn spotlight_launcher_is_embedded_not_an_overlay_window() {
        let theme = load_named(builtin::SPOTLIGHT_ID).expect("spotlight should resolve");
        assert_eq!(theme.launcher().mode, LauncherMode::Embedded);
        assert_eq!(theme.slots().centre, vec!["launcher_entry"]);
        assert_eq!(theme.slots().drawer, vec!["launcher_results"]);
        assert!(
            !theme.slots().drawer.is_empty(),
            "spotlight is the one builtin that actually uses the drawer slot"
        );
    }

    #[test]
    fn spotlight_launcher_widget_slot_gives_left_of_stats_widgets_a_home() {
        let theme = load_named(builtin::SPOTLIGHT_ID).expect("spotlight should resolve");
        assert!(
            theme
                .slots()
                .right
                .iter()
                .any(|s| s == "widget:left_of_stats"),
            "a Lua widget requesting WidgetPlacement::LeftOfStats (e.g. \
             git-branch-widget.lua) must have a home under spotlight: {:?}",
            theme.slots().right
        );
    }

    #[test]
    fn spotlight_query_modes_and_sections_are_enabled_phase_6c() {
        let theme = load_named(builtin::SPOTLIGHT_ID).expect("spotlight should resolve");
        let l = theme.launcher();
        assert!(l.sections, "phase 6c: recent/apps headers are implemented");
        for mode in ["apps", "calc", "cmd", "url"] {
            assert!(
                l.modes.iter().any(|m| m == mode),
                "spotlight.modes should list \"{mode}\": {:?}",
                l.modes
            );
        }
    }

    #[test]
    fn spotlight_launcher_search_state_geometry_differs_from_idle() {
        let theme = load_named(builtin::SPOTLIGHT_ID).expect("spotlight should resolve");
        let l = theme.launcher();
        assert_eq!(l.width, 480);
        assert_eq!(l.search_width, 520);
        assert_eq!(l.radius, 22);
        assert_eq!(l.search_radius, 20);
    }

    #[test]
    fn launcher_search_geometry_defaults_to_idle_values_when_unset() {
        // liquid-motion's own [launcher] table never sets search_width/
        // search_radius — a theme that omits them must fall back to "no
        // change while searching", not some hardcoded magic number.
        let theme = resolve_builtin();
        let l = theme.launcher();
        assert_eq!(l.search_width, l.width);
        assert_eq!(l.search_radius, l.radius);
    }

    #[test]
    fn spotlight_workspaces_are_dots_with_the_demos_own_widths() {
        let theme = load_named(builtin::SPOTLIGHT_ID).expect("spotlight should resolve");
        assert!(matches!(theme.modules().workspaces.style, WorkspaceStyle::Dots));
        assert_eq!(theme.modules().workspaces.dot_widths, [6, 10, 14, 18]);
    }

    #[test]
    fn spotlight_clock_is_none_with_placeholder_clock_set() {
        let theme = load_named(builtin::SPOTLIGHT_ID).expect("spotlight should resolve");
        assert!(matches!(theme.modules().clock.style, ClockStyle::None));
        assert!(
            theme.modules().clock.placeholder_clock,
            "spotlight's entry placeholder stands in for a clock module entirely"
        );
    }

    #[test]
    fn spotlight_accent_is_flat_pink_and_a_palette_token_not_hex() {
        let theme = load_named(builtin::SPOTLIGHT_ID).expect("spotlight should resolve");
        let t = theme.tokens();
        assert_eq!(t.accent_from(), "pink");
        assert_eq!(t.accent_to(), "pink");
        let css = theme.css(&crate::Palette::default());
        assert!(
            css.contains("@pink"),
            "accent_from/accent_to must resolve to the @pink palette token, not a hex literal:\n{css}"
        );
        assert!(
            !css.contains("#e87898"),
            "the demo's pink hex must never leak into the manifest — pywal theming depends on \
             this staying a palette token name:\n{css}"
        );
    }

    // ---- extends merge ------------------------------------------------

    #[test]
    fn extends_deep_merges_one_level() {
        let xdg = isolated_xdg();
        write_theme(
            &xdg,
            "base",
            r#"
                name = "Base"
                id = "base"
                [tokens]
                radius_bar = 10
                pad = 8
                [bar.window]
                height = 40
                [bar.slots]
                left = ["workspaces"]
                right = ["battery"]
            "#,
        );
        write_theme(
            &xdg,
            "child",
            r#"
                name = "Child"
                id = "child"
                extends = "base"
                [tokens]
                radius_bar = 99
                [bar.slots]
                right = ["wifi", "battery"]
            "#,
        );

        let theme = load_named("child").expect("child theme should resolve");
        // Overridden by the child.
        assert_eq!(theme.tokens().radius_bar(), 99);
        // Inherited from the base, untouched by the child.
        assert_eq!(theme.tokens().pad(), 8);
        assert_eq!(theme.window().height, 40);
        // Arrays replace wholesale, not concatenate.
        assert_eq!(theme.slots().right, vec!["wifi", "battery"]);
        // A key the child never mentions at all stays inherited.
        assert_eq!(theme.slots().left, vec!["workspaces"]);
    }

    #[test]
    fn extends_chain_is_capped_at_one_level() {
        let xdg = isolated_xdg();
        write_theme(
            &xdg,
            "grandparent",
            r#"
                id = "grandparent"
                [tokens]
                pad = 1
            "#,
        );
        write_theme(
            &xdg,
            "parent",
            r#"
                id = "parent"
                extends = "grandparent"
                [tokens]
                pad = 2
            "#,
        );
        write_theme(
            &xdg,
            "child",
            r#"
                id = "child"
                extends = "parent"
            "#,
        );

        let theme = load_named("child").expect("child theme should resolve");
        // Only one level is honored: child merges with parent (pad=2), and
        // parent's own `extends = "grandparent"` is dropped rather than
        // chased — the grandparent's pad=1 never applies.
        assert_eq!(theme.tokens().pad(), 2);
    }

    // ---- validation ------------------------------------------------------

    #[test]
    fn unknown_key_error_names_the_offending_key() {
        let xdg = isolated_xdg();
        write_theme(
            &xdg,
            "typo",
            r#"
                id = "typo"
                [bar.window]
                heihgt = 44
            "#,
        );
        let err = load_named("typo").expect_err("typo'd key must be a hard error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("heihgt"),
            "error should name the bad key, got: {msg}"
        );
    }

    #[test]
    fn unknown_top_level_key_is_an_error() {
        let xdg = isolated_xdg();
        write_theme(&xdg, "typo2", "id = \"typo2\"\nfont = \"nope\"\n");
        let err = load_named("typo2").expect_err("unknown top-level key must be a hard error");
        assert!(format!("{err:#}").contains("font"));
    }

    #[test]
    fn unknown_slot_module_names_the_module() {
        let xdg = isolated_xdg();
        write_theme(
            &xdg,
            "badmodule",
            r#"
                id = "badmodule"
                [bar.slots]
                left = ["teleporter"]
            "#,
        );
        let err = load_named("badmodule").expect_err("unknown module must be a hard error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("teleporter"),
            "error should name the module, got: {msg}"
        );
    }

    #[test]
    fn widget_prefixed_slot_entries_are_always_valid() {
        let xdg = isolated_xdg();
        write_theme(
            &xdg,
            "widgetslot",
            r#"
                id = "widgetslot"
                [bar.slots]
                left = ["widget:my-lua-module"]
            "#,
        );
        let theme = load_named("widgetslot").expect("widget: entries should validate");
        assert_eq!(theme.slots().left, vec!["widget:my-lua-module"]);
    }

    #[test]
    fn invalid_enum_value_is_an_error() {
        let xdg = isolated_xdg();
        write_theme(
            &xdg,
            "badstyle",
            r#"
                id = "badstyle"
                [modules.workspaces]
                style = "hexagon"
            "#,
        );
        let err = load_named("badstyle").expect_err("invalid style must be an error");
        assert!(format!("{err:#}").contains("hexagon"));
    }

    #[test]
    fn unknown_surface_anchor_is_a_hard_error_not_a_silent_default() {
        // Before the fix, `surfaces.*.anchor` was the one enum-shaped
        // field in the whole schema that skipped manifest-time validation
        // entirely (`unwrap_or_default()`), unlike its siblings `width` and
        // `layer` which both `bail!`. A typo here must fail exactly like
        // any other typo'd enum value.
        let xdg = isolated_xdg();
        write_theme(
            &xdg,
            "badanchor",
            r#"
                id = "badanchor"
                [surfaces."breadbar-notif"]
                anchor = "top_lft"
                width  = 320
                layer  = "overlay"
            "#,
        );
        let err = load_named("badanchor").expect_err("unknown anchor must be a hard error");
        assert!(format!("{err:#}").contains("top_lft"));
    }

    #[test]
    fn missing_surface_anchor_is_a_hard_error() {
        let xdg = isolated_xdg();
        write_theme(
            &xdg,
            "noanchor",
            r#"
                id = "noanchor"
                [surfaces."breadbar-notif"]
                width  = 320
                layer  = "overlay"
            "#,
        );
        let err = load_named("noanchor").expect_err("missing anchor must be a hard error");
        assert!(format!("{err:#}").contains("no anchor set"));
    }

    #[test]
    fn every_known_surface_anchor_shape_resolves() {
        for anchor in ["top_right", "bottom_centre", "fill"] {
            let xdg = isolated_xdg();
            write_theme(
                &xdg,
                "okanchor",
                &format!(
                    r#"
                        id = "okanchor"
                        [surfaces."breadbar-notif"]
                        anchor = "{anchor}"
                    "#
                ),
            );
            let theme = load_named("okanchor")
                .unwrap_or_else(|e| panic!("anchor {anchor} should resolve: {e:#}"));
            assert_eq!(theme.surfaces()["breadbar-notif"].anchor, anchor);
        }
    }

    // ---- fallback on broken theme -----------------------------------------

    #[test]
    fn malformed_active_theme_falls_back_to_builtin_without_panicking() {
        let xdg = isolated_xdg();
        // A broken override sitting at the *active* id's own path — this is
        // the case resolve_builtin() exists to survive without re-touching
        // the same broken file.
        write_theme(&xdg, "liquid-motion", "this is not [valid toml");

        let theme = load(); // must not panic
        assert_eq!(theme.window().height, 44);
        assert_eq!(theme.id(), "liquid-motion");
    }

    #[test]
    fn missing_active_theme_falls_back_to_builtin() {
        let xdg = isolated_xdg();
        std::env::set_var("BREAD_SHELL_THEME", "does-not-exist");
        let theme = load();
        assert_eq!(theme.id(), "liquid-motion");
        drop(xdg);
    }

    #[test]
    fn load_named_on_broken_theme_returns_err_not_panic() {
        let xdg = isolated_xdg();
        write_theme(&xdg, "broken", "not = [valid");
        assert!(load_named("broken").is_err());
    }

    // ---- css() / extra.css -------------------------------------------------

    #[test]
    fn extra_css_overlay_is_appended_and_token_substituted() {
        let xdg = isolated_xdg();
        let dir = xdg.dir.join("bread/themes/overlaid");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("theme.toml"),
            r#"
                id = "overlaid"
                extends = "liquid-motion"
                css = "extra.css"
                [tokens]
                pad = 21
            "#,
        )
        .unwrap();
        std::fs::write(dir.join("extra.css"), ".custom { margin: {pad}px; }\n").unwrap();

        let theme = load_named("overlaid").expect("theme with extra.css should resolve");
        let css = theme.css(&crate::Palette::default());
        assert!(
            css.contains(".custom { margin: 21px; }"),
            "overlay not appended/substituted:\n{css}"
        );
        // The inherited liquid-motion template should still be present too.
        assert!(css.contains("window.breadbar"));
    }

    // ---- active theme resolution -------------------------------------------

    #[test]
    fn env_var_overrides_shell_toml_active() {
        let xdg = isolated_xdg();
        std::fs::create_dir_all(xdg.dir.join("bread")).unwrap();
        std::fs::write(
            xdg.dir.join("bread/shell.toml"),
            "active = \"liquid-motion\"\n",
        )
        .unwrap();
        write_theme(&xdg, "envwins", "id = \"envwins\"\n");
        std::env::set_var("BREAD_SHELL_THEME", "envwins");
        assert_eq!(active_theme_id(), "envwins");
    }

    #[test]
    fn shell_toml_active_used_when_no_env_var() {
        let xdg = isolated_xdg();
        std::fs::create_dir_all(xdg.dir.join("bread")).unwrap();
        std::fs::write(xdg.dir.join("bread/shell.toml"), "active = \"from-file\"\n").unwrap();
        assert_eq!(active_theme_id(), "from-file");
    }

    #[test]
    fn defaults_to_liquid_motion_with_nothing_configured() {
        let _xdg = isolated_xdg();
        assert_eq!(active_theme_id(), "liquid-motion");
    }

    // ---- list() ------------------------------------------------------------

    #[test]
    fn list_includes_user_themes_and_the_builtin() {
        let xdg = isolated_xdg();
        write_theme(
            &xdg,
            "custom-one",
            "id = \"custom-one\"\nname = \"Custom One\"\n",
        );
        let summaries = list();
        assert!(summaries
            .iter()
            .any(|s| s.id == "custom-one" && s.source == ThemeSource::User));
        assert!(summaries
            .iter()
            .any(|s| s.id == "liquid-motion" && s.source == ThemeSource::Builtin));
    }

    #[test]
    fn list_user_theme_shadows_builtin_of_the_same_id() {
        let xdg = isolated_xdg();
        write_theme(
            &xdg,
            "liquid-motion",
            "id = \"liquid-motion\"\nname = \"User Override\"\n",
        );
        let summaries = list();
        let matches: Vec<_> = summaries
            .iter()
            .filter(|s| s.id == "liquid-motion")
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "same id must not appear twice: {summaries:?}"
        );
        assert_eq!(matches[0].source, ThemeSource::User);
    }
}
