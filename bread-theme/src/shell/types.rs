//! Fully-resolved shell theme types — see `bread-theme/src/shell/mod.rs` for
//! the module overview and `manifest.rs` for how these are built from TOML.
//!
//! Every type here has all defaults filled in; there is no further "is this
//! set" branching once a `ShellTheme` exists. That resolution work happens
//! once, in `manifest.rs`, so consumers (breadbar, breadbox, bos-settings)
//! never have to know the manifest format at all.

use std::collections::BTreeMap;

/// Workspace strip rendering. Phase 1 ships only `Trail` (what breadbar draws
/// today); `Pill`/`Dots` exist now so 02/04 (plan §11 phases 5-6) are additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceStyle {
    Trail,
    Pill,
    Dots,
}

/// Clock rendering. Phase 1 ships only `Flip` (today's per-digit flip clock).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockStyle {
    Flip,
    Plain,
    None,
}

/// Dot pill widths (px) for 0/1/2/3-or-more open windows, `style = "dots"`
/// (theme 04/spotlight). Index 3 covers "3 or more" — the demo's own dots
/// never grow past that fourth width. Unused by `Trail`/`Pill`.
pub type DotWidths = [i32; 4];

/// The demo's own numbers (`04-spotlight.html`'s `.dots button[data-n="N"]`
/// rules) — the default a theme gets if it sets `style = "dots"` but omits
/// `dot_widths`.
pub const DEFAULT_DOT_WIDTHS: DotWidths = [6, 10, 14, 18];

/// How the launcher attaches to the shell. Phase 1 ships only `Overlay`
/// (breadbox's own window); `Embedded` is theme 04's bar-drawer launcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherMode {
    Overlay,
    Embedded,
}

/// `gtk4_layer_shell::KeyboardMode` mirror, kept independent of the `gtk`
/// feature so the manifest types stay usable without GTK linked in (bread,
/// breadcrumbs). Values map 1:1 onto `KeyboardMode::{None,Exclusive,OnDemand}`
/// (verified against gtk4-layer-shell 0.8.1's `src/auto/enums.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyboard {
    None,
    OnDemand,
    Exclusive,
}

/// `bar.window.width` / a surface's `width`: `"fill"` spans the anchored
/// edges, a bare number is a fixed/centred/hug width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Width {
    Fill,
    Px(i32),
}

/// `bar.window.exclusive`: `"auto"` reserves `height + margin.top`, `"none"`
/// reserves nothing (theme 04's capsule), or a literal pixel override.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Exclusive {
    Auto,
    None,
    Px(i32),
}

/// A satellite surface's width: unlike the bar window, a satellite can also
/// be `Auto` — sized by its own content/CSS with no `set_default_width` call
/// at all. `breadbar-panel` is exactly this today (popover content decides
/// its width via `.control-panel-inner`/`.wifi-popover-inner` min-width, not
/// the layer-shell window).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SurfaceWidth {
    Fill,
    Auto,
    Px(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Margin {
    pub top: i32,
    pub left: i32,
    pub right: i32,
    pub bottom: i32,
}

/// `bar.window` — plan §2: window shape is data, not a closed layout enum.
/// Island/Edge/Capsule are three *values* of this struct, not three code
/// paths.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowSpec {
    pub anchors: Vec<String>,
    pub width: Width,
    pub height: i32,
    pub margin: Margin,
    pub exclusive: Exclusive,
    pub keyboard: Keyboard,
    pub layer: String,
}

impl Default for WindowSpec {
    /// Generic baseline for a theme that omits `[bar.window]` entirely —
    /// deliberately the plan §4 schema example's numbers, not necessarily
    /// any particular shipped theme's. `liquid-motion` sets every field
    /// explicitly, so it never falls through to this.
    fn default() -> Self {
        WindowSpec {
            anchors: vec!["top".into(), "left".into(), "right".into()],
            width: Width::Fill,
            height: 44,
            margin: Margin {
                top: 12,
                left: 14,
                right: 14,
                bottom: 0,
            },
            exclusive: Exclusive::Auto,
            keyboard: Keyboard::None,
            layer: "top".into(),
        }
    }
}

/// `bar.slots` — plan §2: structure is slots, not layout code. `drawer` is
/// the only thing Capsule/theme-04 adds over Island, and it's just an
/// (empty, for now) slot list, not a code path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Slots {
    pub left: Vec<String>,
    pub centre: Vec<String>,
    pub right: Vec<String>,
    pub drawer: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacesModule {
    pub style: WorkspaceStyle,
    pub show_empty: bool,
    /// `style = "dots"` only — see [`DotWidths`]. Trail/Pill ignore this.
    pub dot_widths: DotWidths,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockModule {
    pub style: ClockStyle,
    pub format: String,
    pub show_date: bool,
    /// `style = "none"` + this `true`: no module renders a clock label of
    /// its own — `launcher_entry`'s placeholder text becomes the time
    /// instead (theme 04/spotlight: the capsule's entry IS the clock until
    /// focused, per `04-spotlight.html`'s `q.placeholder = t`). Meaningless
    /// for `Flip`/`Plain`, which already show a time some other way.
    pub placeholder_clock: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modules {
    pub workspaces: WorkspacesModule,
    pub clock: ClockModule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    pub mode: LauncherMode,
    pub width: i32,
    pub top: String,
    pub radius: i32,
    pub icon_px: i32,
    pub row_anim: String,
    pub rule: String,
    pub footer: String,
    pub sections: bool,
    pub modes: Vec<String>,
    /// `LauncherMode::Embedded` only (theme 04/spotlight, plan §7 phase 6c):
    /// the capsule's own width while a search is in progress
    /// (`04-spotlight.html`: `.searching .capsule { width: 520px }` vs the
    /// idle 480px). Defaults to `width` (no widen) for a theme that omits
    /// it, so an `Overlay`-mode theme — which never reads this field at all
    /// — and an `Embedded` theme that just doesn't want the widen both fall
    /// back to "no change" rather than a hardcoded magic number.
    pub search_width: i32,
    /// `LauncherMode::Embedded` only: `border-radius` while searching
    /// (`04-spotlight.html`: `.searching .capsule { border-radius: 20px }`
    /// vs the collapsed 22px `radius`). Same default-to-`radius` fallback
    /// reasoning as `search_width`.
    pub search_radius: i32,
}

/// A satellite surface, keyed by layer-shell namespace in `[surfaces.*]` —
/// deliberately the same keyspace as `[compositor.*]` (see module docs)
/// rather than a role name, so the two tables can be validated against each
/// other and a namespace's positioning and compositor treatment live under
/// one lookup.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub anchor: String,
    pub offset: Vec<f64>,
    pub width: SurfaceWidth,
    pub layer: String,
}

/// One `[compositor.*]` entry — plan §9: the per-namespace layer-shell rule
/// an app ships as its default and a theme may override. Mirrors the field
/// set `hl.layer_rule` actually accepts in `scripts/ui/rules.lua` (blur,
/// ignore_alpha, blur_popups, animation, no_anim) — that Lua API isn't in
/// hyprland-api.lua's type annotations, so this field set is evidenced by
/// working usage, not documentation (plan §12 risk 3).
///
/// Also `Serialize`: this is the per-namespace shape written to
/// `~/.config/hypr/layerrules.json` by `bread_theme::layerrules` (plan §9
/// step 3-4), which `scripts/ui/rules.lua` parses back into `hl.layer_rule`
/// calls. `Option::None` fields are omitted rather than emitted as `null` —
/// the Lua JSON reader treats a missing key and a `null` value identically
/// (assigning `nil` into a table key is a no-op), so either encoding is
/// correct, but omitting keeps the file legible for hand inspection.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct LayerRule {
    pub blur: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_alpha: Option<f64>,
    pub blur_popups: bool,
    /// Passed through verbatim to `hl.layer_rule`'s `animation` field
    /// (`"slide top"`, `"slide bottom"`, …) — kept as a plain string rather
    /// than a closed enum since `hl.layer_rule`'s own field set is only
    /// evidenced by working usage in `rules.lua`, not documented (plan §12
    /// risk 3); a closed Rust enum here would need updating in lockstep
    /// with Hyprland additions this crate has no way to know about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub animation: Option<String>,
    pub no_anim: bool,
}

/// A raw TOML scalar carried through to [`Tokens`] for `{name}` substitution
/// in [`crate::shell::ShellTheme::css`]. Kept untyped (rather than forcing
/// every token into a `String`) so `css()` can format a number without a
/// theme author having to quote it, while `bg_alpha = 0.72` etc. still round
/// -trips as a real float for any future non-string consumer.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenValue {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl TokenValue {
    /// Textual form used both for `{name}` substitution in CSS and for the
    /// typed accessors' fallback formatting.
    pub fn as_css(&self) -> String {
        match self {
            TokenValue::Str(s) => s.clone(),
            TokenValue::Int(i) => i.to_string(),
            TokenValue::Float(f) => {
                if f.fract() == 0.0 {
                    format!("{f:.0}")
                } else {
                    f.to_string()
                }
            }
            TokenValue::Bool(b) => b.to_string(),
        }
    }
}

/// `[tokens]`, resolved. Deliberately an open bag (`BTreeMap`), not a fixed
/// struct: the schema (plan §4) names eleven fields, but a theme may define
/// arbitrary extra keys purely for `{name}` substitution in `extra.css`
/// (`radius_pill`, `chip_height`, `icon_px`, `spring_settle` below are all
/// exactly this — real values `theme.rs::load_css` uses today that the plan
/// text's `[tokens]` example didn't list). The named accessors below give
/// the documented fields typed access with sensible defaults; [`Tokens::get`]
/// and [`Tokens::substitute`] cover everything else.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tokens {
    pub(crate) map: BTreeMap<String, TokenValue>,
}

impl Tokens {
    pub fn from_map(map: BTreeMap<String, TokenValue>) -> Self {
        Tokens { map }
    }

    pub fn get(&self, key: &str) -> Option<&TokenValue> {
        self.map.get(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(|s| s.as_str())
    }

    fn str_or(&self, key: &str, default: &str) -> String {
        match self.map.get(key) {
            Some(v) => v.as_css(),
            None => default.to_string(),
        }
    }

    fn int_or(&self, key: &str, default: i64) -> i64 {
        match self.map.get(key) {
            Some(TokenValue::Int(i)) => *i,
            Some(TokenValue::Float(f)) => *f as i64,
            Some(TokenValue::Str(s)) => s.parse().unwrap_or(default),
            _ => default,
        }
    }

    fn float_or(&self, key: &str, default: f64) -> f64 {
        match self.map.get(key) {
            Some(TokenValue::Float(f)) => *f,
            Some(TokenValue::Int(i)) => *i as f64,
            Some(TokenValue::Str(s)) => s.parse().unwrap_or(default),
            _ => default,
        }
    }

    pub fn font_family(&self) -> String {
        self.str_or("font_family", crate::tokens::FONT_FAMILY)
    }
    pub fn font_fallback(&self) -> String {
        self.str_or("font_fallback", "sans-serif")
    }
    pub fn font_size_base(&self) -> i64 {
        self.int_or("font_size_base", crate::tokens::FONT_SIZE_BASE as i64)
    }
    pub fn radius_bar(&self) -> i64 {
        self.int_or("radius_bar", crate::tokens::RADIUS_PRIMARY as i64)
    }
    pub fn radius_card(&self) -> i64 {
        self.int_or("radius_card", crate::tokens::RADIUS_PRIMARY as i64)
    }
    pub fn radius_sm(&self) -> i64 {
        self.int_or("radius_sm", crate::tokens::RADIUS_SECONDARY as i64)
    }
    /// Not in the plan §4 schema list, but a named local in
    /// `theme.rs::load_css` (`radius_pill = "999px"`) alongside the three
    /// siblings that are. See the [`Tokens`] doc comment.
    pub fn radius_pill(&self) -> i64 {
        self.int_or("radius_pill", crate::tokens::RADIUS_PILL as i64)
    }
    pub fn pad(&self) -> i64 {
        self.int_or("pad", crate::tokens::SPACE_MD as i64)
    }
    pub fn bg_alpha(&self) -> f64 {
        self.float_or("bg_alpha", 0.72)
    }
    /// The overshoot/bounce curve (`0.22, 1.35, 0.36, 1`) — clock flips,
    /// pop-ins, the workspace caret draw.
    pub fn spring(&self) -> String {
        self.str_or("spring", "cubic-bezier(0.22, 1.35, 0.36, 1)")
    }
    /// The settle curve (`0.22, 1.2, 0.36, 1`) — hovers, workspace-btn
    /// opacity/background transitions, OSD/notification slide-ins. Not in
    /// the plan §4 schema (which names only `spring`), but `theme.rs` uses
    /// it just as pervasively as the overshoot curve. See [`Tokens`] doc.
    pub fn spring_settle(&self) -> String {
        self.str_or("spring_settle", "cubic-bezier(0.22, 1.2, 0.36, 1)")
    }
    pub fn accent_from(&self) -> String {
        self.str_or("accent_from", "accent")
    }
    pub fn accent_to(&self) -> String {
        let from = self.accent_from();
        self.str_or("accent_to", &from)
    }
    /// Workspace-pill / chip height. Not in the plan §4 schema, but
    /// `breadbar::CHIP_HEIGHT` (32) today. See [`Tokens`] doc.
    pub fn chip_height(&self) -> i64 {
        self.int_or("chip_height", 32)
    }
    /// Not in the plan §4 schema, but `breadbar::ICON_PX` (24) today. See
    /// [`Tokens`] doc.
    pub fn icon_px(&self) -> i64 {
        self.int_or("icon_px", 24)
    }
    /// Not in the plan §4 schema. `"full"` (default, liquid-motion's island)
    /// draws a border on all four edges; `"bottom"` (glass-workbench's flush
    /// edge-to-edge bar, plan §1) draws only the bottom hairline the demo's
    /// `.bar { border-bottom: 1px solid #ffffff12 }` calls for — a floating
    /// island's full border would otherwise render as a stray top/side line
    /// flush against the screen edge. See [`Tokens`] doc; consumed by
    /// `breadbar::theme::load_css`, not by [`crate::shell::ShellTheme::css`].
    pub fn bar_border(&self) -> String {
        self.str_or("bar_border", "full")
    }

    /// Replace every `{name}` occurrence in `template` with that token's
    /// [`TokenValue::as_css`] form. Longest names are substituted first
    /// (mirrors [`crate::resolve_color_names`]) so `{radius}` cannot
    /// half-consume `{radius_bar}` if a theme happens to define both.
    /// `@name` palette references are untouched — this only ever looks at
    /// `{...}` tokens.
    pub fn substitute(&self, template: &str) -> String {
        let mut keys: Vec<&String> = self.map.keys().collect();
        keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
        let mut out = template.to_string();
        for k in keys {
            let value = self.map[k].as_css();
            out = out.replace(&format!("{{{k}}}"), &value);
        }
        out
    }
}
