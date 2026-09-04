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
    /// Applied by breadbar via `LayerShell::set_margin(Edge::Bottom, …)` and
    /// folded into the exclusive zone for a bottom-anchored bar (see
    /// `breadbar::exclusive_zone_for`). daylight is the first built-in to set
    /// it (a bottom-anchored floating dock).
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
    /// Consumed only by `ClockStyle::Plain` (`breadbar::bar::clock`'s
    /// `formatted()`, called from `main.rs` only in the `Plain` arm) —
    /// `Flip` and `None` never read it; `Flip`'s own `time()` hardcodes a
    /// 24h `HH:MM` layout regardless of this field. All three built-in
    /// themes set `format = "%H:%M"` (which happens to match `Flip`'s
    /// hardcoded layout, masking the gap for two of the three styles); see
    /// each `theme.toml`'s own note next to it for which styles actually
    /// honour this.
    pub format: String,
    /// Consumed only by `ClockStyle::Plain`, same scoping as
    /// [`Self::format`] — `date_lbl` is only ever attached under the
    /// `Plain` arm's box, so this value can never be observed under
    /// `Flip`/`None`. Currently harmless in practice (every built-in theme
    /// that isn't `Plain` sets this `false`, so there's nothing to ignore),
    /// but the same "declared, scoped to one style" caveat as `format`
    /// applies.
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

// No `Eq`: `selection_alpha` is an `f64`, which doesn't implement it.
#[derive(Debug, Clone, PartialEq)]
pub struct Launcher {
    pub mode: LauncherMode,
    pub width: i32,
    pub top: String,
    pub radius: i32,
    pub icon_px: i32,
    pub row_anim: String,
    pub rule: String,
    /// Selects the footer label's noun — `"count_apps"` renders
    /// `"{n} applications"`, anything else (`"count_results"` included)
    /// renders `"{n} results"`. Consumed by `breadbox::main`'s footer label
    /// (appended below `ResultsList::scroller`, updated on every
    /// `set_query` call alongside the visible-row count).
    pub footer: String,
    /// Consumed by breadbar's embedded capsule (`breadbar::main::run_ui`,
    /// passed straight to `ResultsList::new`) for every theme, and by
    /// breadbox's own overlay (`breadbox::main::run_ui`) as of the
    /// liquid-motion/glass-workbench redesign — `true` groups the idle
    /// (empty-query) view into "Recent"/"Apps" headers via
    /// `bread_launcher::gtk::split_sections`, `false` reproduces the flat
    /// list. breadbox no longer hardcodes `false` at its `ResultsList::new`
    /// call site; it reads this field.
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
    /// Result row `border-radius` (px) — `breadbox::main::build_css`'s
    /// `row { border-radius: ... }`. Liquid Motion's soft 12px vs Glass
    /// Workbench's dense 6px is the clearest single signal that the two
    /// themes are different instruments, not one launcher recoloured.
    pub row_radius: i32,
    /// Result row horizontal inset (px) from the panel's edge —
    /// `row { margin: 0 {row_inset}px; }`. Mirrors the demos' `.bx .r`
    /// `margin: 0 Npx` rule (liquid-motion 8px, glass-workbench 6px).
    pub row_inset: i32,
    /// Result row vertical padding (px) — `row { padding: {row_padding_v}px
    /// {row_padding_h}px; }`'s first component.
    pub row_padding_v: i32,
    /// Result row horizontal padding (px) — same rule's second component.
    pub row_padding_h: i32,
    /// Row icon `border-radius` (px) — `breadbox::main::build_css`'s
    /// `image { border-radius: ...; }`, paired with `icon_px` for the
    /// swatch's size. Liquid Motion's rounder 9px vs Glass Workbench's
    /// tighter 5px.
    pub icon_radius: i32,
    /// Search entry font-size (px) — distinct from `tokens.font_size_base`
    /// (which sizes the result rows): both demos give the search field a
    /// larger face than its rows (liquid-motion 16 vs its rows' 14,
    /// glass-workbench 14 vs its rows' 13).
    pub search_font_size: i32,
    /// Search entry vertical padding (px).
    pub search_padding_v: i32,
    /// Search entry horizontal padding (px).
    pub search_padding_h: i32,
    /// Opacity of the launcher PANEL itself, distinct from `tokens.bg_alpha`
    /// (which governs the thin bar). A bar can be very translucent and stay
    /// readable because it is 36-44px tall over a small slice of wallpaper; a
    /// full launcher panel at the same alpha washes out badly over a bright
    /// wallpaper and its text becomes hard to read. The approved reference
    /// uses 0.95 (glass-workbench) and 0.93 (liquid-motion); breadbox
    /// previously hardcoded 0.60, which is what made it look washed out.
    pub panel_alpha: f64,
    /// Selected/hovered row background: `alpha(@accent, selection_alpha)`.
    /// Liquid Motion's softer 0.22 vs Glass Workbench's denser 0.28 — see
    /// each demo's `.bx .r.sel` rule.
    pub selection_alpha: f64,
}

/// A satellite surface, keyed by layer-shell namespace in `[surfaces.*]` —
/// deliberately the same keyspace as `[compositor.*]` (see module docs)
/// rather than a role name, so the two tables can be validated against each
/// other and a namespace's positioning and compositor treatment live under
/// one lookup.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    /// One of `top_right`/`bottom_right`/`bottom_centre`/`fill` — validated
    /// in `manifest.rs::resolve_surfaces` against the shapes
    /// `breadbar/src/surface.rs::apply` actually implements, the same
    /// "typo'd key is a hard error" policy this field's siblings
    /// (`width`, `layer`) already got. Required, not defaulted: a surface
    /// entry with no anchor at all is as much a hard `theme.toml` error as
    /// an unrecognized one.
    ///
    /// `bottom_right` (daylight, plan §11 phase 7) added alongside the
    /// original three: every built-in theme before it anchored its bar to
    /// the TOP, so `breadbar-notif`/`breadbar-panel` popping up from
    /// `top_right` always sat naturally close to the bar. A bottom-anchored
    /// bar has no shape in the original three that keeps those satellites
    /// near it — `top_right` would put them at the opposite corner of the
    /// screen from the dock they visually belong to. `offset` is
    /// `[right, bottom]` for this anchor, the same two-element convention
    /// `top_right`'s `[right, top]` already uses.
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

/// `bar_border` — how `window.breadbar`'s own surface is drawn. A closed enum,
/// validated at manifest-resolution time like every other string enum in this
/// schema, rather than the free-form string it used to be (a typo silently
/// rendered as `Full`).
///
/// - `Full` — a floating island: fill + a border on all four edges
///   (liquid-motion).
/// - `Bottom` — a flush edge-to-edge bar: fill + only a bottom hairline, since
///   an island's full border would show as a stray line flush against the
///   screen edge (glass-workbench).
/// - `Segmented` — `window.breadbar` itself is fully transparent (no fill,
///   border, radius or shadow); the bar's slot-group containers each carry
///   their own pill surface instead, so one window reads as three detached
///   floating pills (daylight).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BarBorder {
    #[default]
    Full,
    Bottom,
    Segmented,
}

impl BarBorder {
    pub fn as_str(self) -> &'static str {
        match self {
            BarBorder::Full => "full",
            BarBorder::Bottom => "bottom",
            BarBorder::Segmented => "segmented",
        }
    }

    pub(crate) fn parse(theme_id: &str, s: &str) -> anyhow::Result<Self> {
        match s {
            "full" => Ok(BarBorder::Full),
            "bottom" => Ok(BarBorder::Bottom),
            "segmented" => Ok(BarBorder::Segmented),
            other => Err(anyhow::anyhow!(
                "theme '{theme_id}': tokens.bar_border = \"{other}\" is not full|bottom|segmented"
            )),
        }
    }
}

impl std::fmt::Display for BarBorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `[tokens]`, fully resolved: every documented token is a typed field with a
/// default (from [`crate::tokens`]), validated when the manifest is parsed
/// (`manifest::resolve_tokens`). A theme may still define *extra* keys purely
/// for `{name}` substitution in its `extra.css` overlay — those land in
/// [`Tokens::extra`]. A `{name}` in a CSS template that matches neither a
/// field nor an extra key is a hard error at resolve time (see
/// `RawManifest::resolve`), so a typo in a token name can no longer be a
/// silent no-op the way it was when this was an open `BTreeMap`.
///
/// The zero-argument accessor methods (`tokens.radius_bar()` …) are retained
/// as thin getters so consumer call sites (`breadbar`, `breadbox`) don't churn.
#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    pub radius_bar: i64,
    pub radius_card: i64,
    pub radius_sm: i64,
    pub radius_pill: i64,
    pub pad: i64,
    pub bg_alpha: f64,
    pub spring: String,
    pub spring_settle: String,
    pub accent_from: String,
    pub accent_to: String,
    pub accent2: String,
    pub chip_height: i64,
    pub icon_px: i64,
    pub font_family: String,
    pub font_fallback: String,
    pub font_size_base: i64,
    /// CSS `font-weight` for the bar / launcher / shared component text
    /// (100–900). Default 400. Widget-level `.bread-weight-*` classes still
    /// override per element.
    pub font_weight: i64,
    pub light: bool,
    pub bar_border: BarBorder,
    // ---- chrome (CSS-only; nothing in Rust reads these) ------------------
    /// Angle (deg) of the Trail workspace gradient (`accent_from → accent_to`).
    pub ws_gradient_angle: i64,
    /// Extra left-margin (px) between adjacent bar chips.
    pub chip_gap: i64,
    /// Divider between bar stat chips: `line` (default) · `none` · `dot`.
    pub sep_style: String,
    /// Drop shadow under the bar / segments: `none` (default) · `soft` · `hard`.
    pub bar_shadow: String,
    /// Media-widget equaliser: `bars` (default, animated) · `none`.
    pub media_eq_style: String,
    /// OSD pill shape: `pill` (default, `radius_pill`) · `bar` (`radius_card`).
    pub osd_style: String,
    /// Open extras — only referenced by `{name}` substitution in a user
    /// theme's `extra.css`. Not part of the documented schema.
    pub extra: BTreeMap<String, TokenValue>,
}

impl Default for Tokens {
    fn default() -> Self {
        use crate::tokens::*;
        Tokens {
            radius_bar: RADIUS_PRIMARY as i64,
            radius_card: RADIUS_PRIMARY as i64,
            radius_sm: RADIUS_SECONDARY as i64,
            radius_pill: RADIUS_PILL as i64,
            pad: SPACE_MD as i64,
            bg_alpha: 0.72,
            spring: "cubic-bezier(0.22, 1.35, 0.36, 1)".to_string(),
            spring_settle: "cubic-bezier(0.22, 1.2, 0.36, 1)".to_string(),
            accent_from: "accent".to_string(),
            accent_to: "accent".to_string(),
            accent2: "accent".to_string(),
            chip_height: 32,
            icon_px: 24,
            font_family: FONT_FAMILY.to_string(),
            font_fallback: "sans-serif".to_string(),
            font_size_base: FONT_SIZE_BASE as i64,
            font_weight: 400,
            light: false,
            bar_border: BarBorder::Full,
            ws_gradient_angle: 90,
            chip_gap: 0,
            sep_style: "line".to_string(),
            bar_shadow: "none".to_string(),
            media_eq_style: "bars".to_string(),
            osd_style: "pill".to_string(),
            extra: BTreeMap::new(),
        }
    }
}

/// Render a float the way a hand-written token would read: `0.72` stays
/// `0.72`, `16.0` collapses to `16`. Mirrors [`TokenValue::as_css`].
pub(super) fn fmt_f64(f: f64) -> String {
    if f.fract() == 0.0 {
        format!("{f:.0}")
    } else {
        f.to_string()
    }
}

/// Replace every `{name}` in `template` with its pair value, longest name
/// first so `{radius}` can't half-consume `{radius_bar}`. `@name` palette
/// references are left untouched. Shared by the token substitution and the
/// [`crate::shell::style`] derivations.
pub(super) fn substitute_pairs(template: &str, mut pairs: Vec<(String, String)>) -> String {
    pairs.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    let mut out = template.to_string();
    for (k, v) in pairs {
        out = out.replace(&format!("{{{k}}}"), &v);
    }
    out
}

/// Every `{name}` still present after substitution — i.e. a placeholder that
/// matched no `[tokens]` field, no derived style value, and no `extra.css`
/// key. Scoped to `{` + a lowercase identifier + `}` with no whitespace, a
/// shape real CSS rule bodies never produce.
pub(super) fn unresolved_refs(rendered: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = rendered.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'{' {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut j = start;
        while j < b.len() && (b[j].is_ascii_lowercase() || b[j].is_ascii_digit() || b[j] == b'_') {
            j += 1;
        }
        if j > start && j < b.len() && b[j] == b'}' {
            let name = rendered[start..j].to_string();
            if !out.contains(&name) {
                out.push(name);
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

impl Tokens {
    pub fn get(&self, key: &str) -> Option<&TokenValue> {
        self.extra.get(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.extra.keys().map(|s| s.as_str())
    }

    /// Every substitutable `(name, css-value)` pair: the typed fields plus any
    /// `extra` keys. Consumed by [`Self::substitute`] and by the
    /// unresolved-`{ref}` check in `RawManifest::resolve`.
    pub(crate) fn subst_pairs(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = vec![
            ("radius_bar".into(), self.radius_bar.to_string()),
            ("radius_card".into(), self.radius_card.to_string()),
            ("radius_sm".into(), self.radius_sm.to_string()),
            ("radius_pill".into(), self.radius_pill.to_string()),
            ("pad".into(), self.pad.to_string()),
            ("bg_alpha".into(), fmt_f64(self.bg_alpha)),
            ("spring".into(), self.spring.clone()),
            ("spring_settle".into(), self.spring_settle.clone()),
            ("accent_from".into(), self.accent_from.clone()),
            ("accent_to".into(), self.accent_to.clone()),
            ("accent2".into(), self.accent2.clone()),
            ("chip_height".into(), self.chip_height.to_string()),
            ("icon_px".into(), self.icon_px.to_string()),
            ("font_family".into(), self.font_family.clone()),
            ("font_fallback".into(), self.font_fallback.clone()),
            ("font_size_base".into(), self.font_size_base.to_string()),
            ("font_weight".into(), self.font_weight.to_string()),
            ("light".into(), self.light.to_string()),
            ("bar_border".into(), self.bar_border.as_str().to_string()),
            (
                "ws_gradient_angle".into(),
                self.ws_gradient_angle.to_string(),
            ),
            ("chip_gap".into(), self.chip_gap.to_string()),
        ];
        for (k, tv) in &self.extra {
            v.push((k.clone(), tv.as_css()));
        }
        v
    }

    // ---- thin field getters (retained so consumer call sites don't churn) ----

    pub fn font_family(&self) -> String {
        self.font_family.clone()
    }
    pub fn font_fallback(&self) -> String {
        self.font_fallback.clone()
    }
    pub fn font_size_base(&self) -> i64 {
        self.font_size_base
    }
    /// CSS `font-weight` (100–900) for bar / launcher / shared component text.
    pub fn font_weight(&self) -> i64 {
        self.font_weight
    }
    pub fn radius_bar(&self) -> i64 {
        self.radius_bar
    }
    pub fn radius_card(&self) -> i64 {
        self.radius_card
    }
    pub fn radius_sm(&self) -> i64 {
        self.radius_sm
    }
    pub fn radius_pill(&self) -> i64 {
        self.radius_pill
    }
    pub fn pad(&self) -> i64 {
        self.pad
    }
    pub fn bg_alpha(&self) -> f64 {
        self.bg_alpha
    }
    /// The overshoot/bounce curve — clock flips, pop-ins, the workspace caret.
    pub fn spring(&self) -> String {
        self.spring.clone()
    }
    /// The settle curve — hovers, opacity/background transitions, slide-ins.
    pub fn spring_settle(&self) -> String {
        self.spring_settle.clone()
    }
    /// Palette token *name* (`"accent"`, `"teal"`, …) — the workspace-trail
    /// gradient's start stop. Resolved against pywal downstream via `@name`.
    pub fn accent_from(&self) -> String {
        self.accent_from.clone()
    }
    /// Gradient end stop; resolved at manifest time to `accent_from` when the
    /// theme omits it (a flat fill).
    pub fn accent_to(&self) -> String {
        self.accent_to.clone()
    }
    /// A second, independent accent (daylight's amber equaliser vs its teal
    /// trail); resolved to `accent_from` when the theme omits it.
    pub fn accent2(&self) -> String {
        self.accent2.clone()
    }
    /// Ink-on-paper surfaces (near-opaque light fills, dark ink) rather than
    /// glass-on-dark. Still read in Rust by `breadbar::theme::fg_color` for
    /// icon-texture tint, which is outside CSS.
    pub fn light(&self) -> bool {
        self.light
    }
    /// Workspace-pill / chip highlight height.
    pub fn chip_height(&self) -> i64 {
        self.chip_height
    }
    pub fn icon_px(&self) -> i64 {
        self.icon_px
    }
    /// How `window.breadbar`'s own surface is drawn — see [`BarBorder`].
    pub fn bar_border(&self) -> BarBorder {
        self.bar_border
    }

    /// Replace every `{name}` in `template` with this token's CSS text form.
    /// `@name` palette references are untouched. The full render path
    /// (`RawManifest::resolve`) also folds in the [`crate::shell::style`]
    /// derivations via [`substitute_pairs`]; this method covers tokens alone.
    pub fn substitute(&self, template: &str) -> String {
        substitute_pairs(template, self.subst_pairs())
    }
}
