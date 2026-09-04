//! `theme.toml` deserialization, validation, and resolution into
//! [`crate::shell::ShellTheme`].
//!
//! Two layers on purpose: `Raw*` types mirror the TOML shape exactly (every
//! field optional, `deny_unknown_fields` everywhere so a typo'd key is a
//! hard error naming that key rather than a silent no-op) and know nothing
//! about defaults; [`RawManifest::resolve`] is the one place defaults get
//! filled and string enums get validated, producing the fully-resolved
//! types in `types.rs`.

use anyhow::{anyhow, bail, Context};
use std::collections::{BTreeMap, HashMap};

use super::types::*;

pub(super) fn validate_module_name(theme_id: &str, slot: &str, module: &str) -> anyhow::Result<()> {
    if module.starts_with("widget:") || super::modules::is_known(module) {
        return Ok(());
    }
    if module == "+" {
        bail!(
            "theme '{theme_id}': slot \"{slot}\" uses the \"+\" splice marker but the theme has \
             no `extends` (or extends a theme with no {slot} slot) — nothing to splice in"
        );
    }
    bail!(
        "theme '{theme_id}': slot \"{slot}\" references unknown module \"{module}\" \
         (known modules: {}, or widget:<lua-module-name>; an app registers its own \
         modules via bread_theme::shell::modules::register)",
        super::modules::all().join(", ")
    );
}

pub(super) fn validate_slots(raw: &RawManifest, theme_id: &str) -> anyhow::Result<()> {
    let Some(bar) = &raw.bar else { return Ok(()) };
    let Some(slots) = &bar.slots else {
        return Ok(());
    };
    for (slot_name, list) in [
        ("left", &slots.left),
        ("centre", &slots.centre),
        ("right", &slots.right),
        ("drawer", &slots.drawer),
    ] {
        for module in list {
            validate_module_name(theme_id, slot_name, module)?;
        }
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawManifest {
    pub(super) name: Option<String>,
    pub(super) id: Option<String>,
    /// Present only so `extends` deserializes as a *known* field (otherwise
    /// `deny_unknown_fields` would reject every theme that sets it). The
    /// value itself is read straight off the raw `toml::Value` in
    /// `mod.rs::resolve_theme` — before this struct exists — since the
    /// merge has to happen ahead of (and separately from) deserialization.
    #[allow(dead_code)]
    pub(super) extends: Option<String>,
    #[serde(default)]
    pub(super) tokens: HashMap<String, toml::Value>,
    pub(super) bar: Option<RawBar>,
    pub(super) modules: Option<RawModules>,
    pub(super) launcher: Option<RawLauncher>,
    pub(super) surfaces: Option<HashMap<String, RawSurface>>,
    pub(super) compositor: Option<HashMap<String, RawLayerRule>>,
    /// Overlay CSS path, resolved relative to the theme file's own directory.
    /// Its contents are token-substituted and appended after the base
    /// template when the theme's CSS is rendered (`RawManifest::resolve` →
    /// `ShellTheme::css`), which breadbar and breadbox now consume directly.
    /// (Schema note: plan §4 shows `css = "extra.css"` textually after the
    /// `[compositor]` table with no header between them, which real TOML
    /// would nest inside `[compositor]`. Treated here as a top-level field
    /// per §5's `css()` doc.)
    pub(super) css: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawBar {
    pub(super) window: Option<RawWindow>,
    pub(super) slots: Option<RawSlots>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawWindow {
    pub(super) anchors: Option<Vec<String>>,
    pub(super) width: Option<RawSize>,
    pub(super) height: Option<i64>,
    pub(super) margin: Option<RawMargin>,
    pub(super) exclusive: Option<RawExclusive>,
    pub(super) keyboard: Option<String>,
    pub(super) layer: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
pub(super) enum RawSize {
    Named(String),
    Px(i64),
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
pub(super) enum RawExclusive {
    Named(String),
    Px(i64),
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(super) struct RawMargin {
    pub(super) top: i64,
    pub(super) left: i64,
    pub(super) right: i64,
    /// See [`super::types::Margin::bottom`] — accepted and resolved, but
    /// breadbar never applies it.
    pub(super) bottom: i64,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(super) struct RawSlots {
    pub(super) left: Vec<String>,
    pub(super) centre: Vec<String>,
    pub(super) right: Vec<String>,
    pub(super) drawer: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawModules {
    pub(super) workspaces: Option<RawWorkspacesModule>,
    pub(super) clock: Option<RawClockModule>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawWorkspacesModule {
    pub(super) style: Option<String>,
    pub(super) show_empty: Option<bool>,
    /// `[6, 10, 14, 18]`-shaped — see [`DotWidths`]. A length other than 4
    /// is a hard error (validated in [`resolve_modules`]) rather than
    /// silently truncated/padded, matching this file's "typo'd key is a
    /// hard error" policy for enum-ish values.
    pub(super) dot_widths: Option<Vec<i64>>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawClockModule {
    pub(super) style: Option<String>,
    pub(super) format: Option<String>,
    pub(super) show_date: Option<bool>,
    pub(super) placeholder_clock: Option<bool>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawLauncher {
    pub(super) mode: Option<String>,
    pub(super) width: Option<i64>,
    pub(super) top: Option<String>,
    pub(super) radius: Option<i64>,
    pub(super) icon_px: Option<i64>,
    pub(super) row_anim: Option<String>,
    pub(super) rule: Option<String>,
    pub(super) footer: Option<String>,
    pub(super) sections: Option<bool>,
    pub(super) modes: Option<Vec<String>>,
    pub(super) search_width: Option<i64>,
    pub(super) search_radius: Option<i64>,
    pub(super) row_radius: Option<i64>,
    pub(super) row_inset: Option<i64>,
    pub(super) row_padding_v: Option<i64>,
    pub(super) row_padding_h: Option<i64>,
    pub(super) icon_radius: Option<i64>,
    pub(super) search_font_size: Option<i64>,
    pub(super) search_padding_v: Option<i64>,
    pub(super) search_padding_h: Option<i64>,
    pub(super) panel_alpha: Option<f64>,
    pub(super) selection_alpha: Option<f64>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawSurface {
    pub(super) anchor: Option<String>,
    pub(super) offset: Option<RawOffset>,
    pub(super) width: Option<RawSurfaceWidth>,
    pub(super) layer: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
pub(super) enum RawOffset {
    Single(f64),
    Pair([f64; 2]),
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
pub(super) enum RawSurfaceWidth {
    Named(String),
    Px(i64),
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(super) struct RawLayerRule {
    pub(super) blur: Option<bool>,
    pub(super) ignore_alpha: Option<f64>,
    pub(super) blur_popups: Option<bool>,
    pub(super) animation: Option<String>,
    pub(super) no_anim: Option<bool>,
}

fn token_value(v: &toml::Value) -> anyhow::Result<TokenValue> {
    match v {
        toml::Value::String(s) => Ok(TokenValue::Str(s.clone())),
        toml::Value::Integer(i) => Ok(TokenValue::Int(*i)),
        toml::Value::Float(f) => Ok(TokenValue::Float(*f)),
        toml::Value::Boolean(b) => Ok(TokenValue::Bool(*b)),
        other => Err(anyhow!("must be a string, number, or bool, got {other:?}")),
    }
}

/// Remove `/* … */` comments from a CSS template. CSS comments do not nest.
/// Run before token substitution so a `{token}` mentioned in an authoring
/// comment is neither substituted nor flagged as an unresolved reference, and
/// so the rendered stylesheet carries no template-authoring prose.
fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The documented `[tokens]` keys — everything else a theme sets under
/// `[tokens]` is an "extra", only meaningful for `{name}` substitution in
/// that theme's own `extra.css` overlay.
const KNOWN_TOKEN_KEYS: &[&str] = &[
    "radius_bar",
    "radius_card",
    "radius_sm",
    "radius_pill",
    "pad",
    "bg_alpha",
    "spring",
    "spring_settle",
    "accent_from",
    "accent_to",
    "accent2",
    "chip_height",
    "icon_px",
    "font_family",
    "font_fallback",
    "font_size_base",
    "font_weight",
    "light",
    "bar_border",
    "ws_gradient_angle",
    "chip_gap",
    "sep_style",
    "bar_shadow",
    "media_eq_style",
    "osd_style",
];

/// Build a fully-resolved, typed [`Tokens`] from the raw `[tokens]` table.
/// Known keys go to their typed field (leniently coerced — a number written
/// as a quoted string still parses, matching the pre-typed behaviour);
/// unknown keys are carried in [`Tokens::extra`] for `extra.css`.
fn resolve_tokens(theme_id: &str, raw: &HashMap<String, toml::Value>) -> anyhow::Result<Tokens> {
    let mut t = Tokens::default();
    let mut accent_to_set = false;
    let mut accent2_set = false;

    let want_i64 = |v: &toml::Value, key: &str| -> anyhow::Result<i64> {
        match v {
            toml::Value::Integer(i) => Ok(*i),
            toml::Value::Float(f) => Ok(*f as i64),
            toml::Value::String(s) => s.parse().with_context(|| {
                format!("theme '{theme_id}': tokens.{key} = \"{s}\" is not a number")
            }),
            other => Err(anyhow!(
                "theme '{theme_id}': tokens.{key} must be a number, got {other:?}"
            )),
        }
    };
    let want_f64 = |v: &toml::Value, key: &str| -> anyhow::Result<f64> {
        match v {
            toml::Value::Float(f) => Ok(*f),
            toml::Value::Integer(i) => Ok(*i as f64),
            toml::Value::String(s) => s.parse().with_context(|| {
                format!("theme '{theme_id}': tokens.{key} = \"{s}\" is not a number")
            }),
            other => Err(anyhow!(
                "theme '{theme_id}': tokens.{key} must be a number, got {other:?}"
            )),
        }
    };
    let want_str = |v: &toml::Value, key: &str| -> anyhow::Result<String> {
        match v {
            toml::Value::String(s) => Ok(s.clone()),
            other => Err(anyhow!(
                "theme '{theme_id}': tokens.{key} must be a string, got {other:?}"
            )),
        }
    };
    let want_enum =
        |v: &toml::Value, key: &str, allowed: &[&str]| -> anyhow::Result<String> {
            let s = match v {
                toml::Value::String(s) => s.clone(),
                other => {
                    return Err(anyhow!(
                        "theme '{theme_id}': tokens.{key} must be a string, got {other:?}"
                    ))
                }
            };
            if allowed.contains(&s.as_str()) {
                Ok(s)
            } else {
                Err(anyhow!(
                    "theme '{theme_id}': tokens.{key} = \"{s}\" is not {}",
                    allowed.join("|")
                ))
            }
        };
    let want_bool = |v: &toml::Value, key: &str| -> anyhow::Result<bool> {
        match v {
            toml::Value::Boolean(b) => Ok(*b),
            toml::Value::String(s) => s.parse().with_context(|| {
                format!("theme '{theme_id}': tokens.{key} = \"{s}\" is not true/false")
            }),
            other => Err(anyhow!(
                "theme '{theme_id}': tokens.{key} must be a bool, got {other:?}"
            )),
        }
    };

    for (k, v) in raw {
        match k.as_str() {
            "radius_bar" => t.radius_bar = want_i64(v, k)?,
            "radius_card" => t.radius_card = want_i64(v, k)?,
            "radius_sm" => t.radius_sm = want_i64(v, k)?,
            "radius_pill" => t.radius_pill = want_i64(v, k)?,
            "pad" => t.pad = want_i64(v, k)?,
            "chip_height" => t.chip_height = want_i64(v, k)?,
            "icon_px" => t.icon_px = want_i64(v, k)?,
            "font_size_base" => t.font_size_base = want_i64(v, k)?,
            "font_weight" => t.font_weight = want_i64(v, k)?,
            "ws_gradient_angle" => t.ws_gradient_angle = want_i64(v, k)?,
            "chip_gap" => t.chip_gap = want_i64(v, k)?,
            "sep_style" => t.sep_style = want_enum(v, k, &["line", "none", "dot"])?,
            "bar_shadow" => t.bar_shadow = want_enum(v, k, &["none", "soft", "hard"])?,
            "media_eq_style" => t.media_eq_style = want_enum(v, k, &["bars", "none"])?,
            "osd_style" => t.osd_style = want_enum(v, k, &["pill", "bar"])?,
            "bg_alpha" => t.bg_alpha = want_f64(v, k)?,
            "spring" => t.spring = want_str(v, k)?,
            "spring_settle" => t.spring_settle = want_str(v, k)?,
            "accent_from" => t.accent_from = want_str(v, k)?,
            "accent_to" => {
                t.accent_to = want_str(v, k)?;
                accent_to_set = true;
            }
            "accent2" => {
                t.accent2 = want_str(v, k)?;
                accent2_set = true;
            }
            "font_family" => t.font_family = want_str(v, k)?,
            "font_fallback" => t.font_fallback = want_str(v, k)?,
            "light" => t.light = want_bool(v, k)?,
            "bar_border" => t.bar_border = BarBorder::parse(theme_id, &want_str(v, k)?)?,
            _ => {
                debug_assert!(!KNOWN_TOKEN_KEYS.contains(&k.as_str()));
                t.extra.insert(
                    k.clone(),
                    token_value(v).with_context(|| format!("theme '{theme_id}': tokens.{k}"))?,
                );
            }
        }
    }

    // A theme that omits `accent_to`/`accent2` gets `accent_from` — a flat
    // fill / a shared second accent, the same fallback the old getters did.
    if !accent_to_set {
        t.accent_to = t.accent_from.clone();
    }
    if !accent2_set {
        t.accent2 = t.accent_from.clone();
    }

    Ok(t)
}

fn resolve_window(theme_id: &str, w: &RawWindow) -> anyhow::Result<WindowSpec> {
    let default = WindowSpec::default();

    let anchors = match &w.anchors {
        Some(list) => {
            for a in list {
                if !matches!(a.as_str(), "top" | "bottom" | "left" | "right") {
                    bail!(
                        "theme '{theme_id}': bar.window.anchors contains unknown anchor \"{a}\" \
                         (expected top|bottom|left|right)"
                    );
                }
            }
            list.clone()
        }
        None => default.anchors,
    };

    let width = match &w.width {
        Some(RawSize::Named(s)) if s == "fill" => Width::Fill,
        Some(RawSize::Named(other)) => bail!(
            "theme '{theme_id}': bar.window.width = \"{other}\" is not \"fill\" \
             (use a bare number for a fixed width)"
        ),
        Some(RawSize::Px(n)) => Width::Px(*n as i32),
        None => default.width,
    };

    let height = w.height.map(|h| h as i32).unwrap_or(default.height);

    let margin = w
        .margin
        .as_ref()
        .map(|m| Margin {
            top: m.top as i32,
            left: m.left as i32,
            right: m.right as i32,
            bottom: m.bottom as i32,
        })
        .unwrap_or(default.margin);

    let exclusive = match &w.exclusive {
        Some(RawExclusive::Named(s)) if s == "auto" => Exclusive::Auto,
        Some(RawExclusive::Named(s)) if s == "none" => Exclusive::None,
        Some(RawExclusive::Named(other)) => bail!(
            "theme '{theme_id}': bar.window.exclusive = \"{other}\" is not \"auto\" or \"none\" \
             (use a bare number for a fixed exclusive zone)"
        ),
        Some(RawExclusive::Px(n)) => Exclusive::Px(*n as i32),
        None => default.exclusive,
    };

    let keyboard = match w.keyboard.as_deref() {
        None => default.keyboard,
        Some("none") => Keyboard::None,
        Some("on_demand") => Keyboard::OnDemand,
        Some("exclusive") => Keyboard::Exclusive,
        Some(other) => bail!(
            "theme '{theme_id}': bar.window.keyboard = \"{other}\" is not none|on_demand|exclusive"
        ),
    };

    let layer = match w.layer.as_deref() {
        None => default.layer,
        Some("top") => "top".to_string(),
        Some("overlay") => "overlay".to_string(),
        Some(other) => {
            bail!("theme '{theme_id}': bar.window.layer = \"{other}\" is not top|overlay")
        }
    };

    Ok(WindowSpec {
        anchors,
        width,
        height,
        margin,
        exclusive,
        keyboard,
        layer,
    })
}

fn resolve_modules(theme_id: &str, m: Option<&RawModules>) -> anyhow::Result<Modules> {
    let ws = m.and_then(|m| m.workspaces.as_ref());
    let style = match ws.and_then(|w| w.style.as_deref()) {
        None => WorkspaceStyle::Trail,
        Some("trail") => WorkspaceStyle::Trail,
        Some("pill") => WorkspaceStyle::Pill,
        Some("dots") => WorkspaceStyle::Dots,
        Some(other) => bail!(
            "theme '{theme_id}': modules.workspaces.style = \"{other}\" is not trail|pill|dots"
        ),
    };
    let show_empty = ws.and_then(|w| w.show_empty).unwrap_or(true);
    let dot_widths = match ws.and_then(|w| w.dot_widths.as_ref()) {
        None => DEFAULT_DOT_WIDTHS,
        Some(v) if v.len() == 4 => [v[0] as i32, v[1] as i32, v[2] as i32, v[3] as i32],
        Some(v) => bail!(
            "theme '{theme_id}': modules.workspaces.dot_widths has {} entries, expected 4 \
             (0/1/2/3-or-more open windows)",
            v.len()
        ),
    };

    let ck = m.and_then(|m| m.clock.as_ref());
    let cstyle = match ck.and_then(|c| c.style.as_deref()) {
        None => ClockStyle::Flip,
        Some("flip") => ClockStyle::Flip,
        Some("plain") => ClockStyle::Plain,
        Some("none") => ClockStyle::None,
        Some(other) => {
            bail!("theme '{theme_id}': modules.clock.style = \"{other}\" is not flip|plain|none")
        }
    };
    let format = ck
        .and_then(|c| c.format.clone())
        .unwrap_or_else(|| "%H:%M".to_string());
    let show_date = ck.and_then(|c| c.show_date).unwrap_or(false);
    let placeholder_clock = ck.and_then(|c| c.placeholder_clock).unwrap_or(false);

    Ok(Modules {
        workspaces: WorkspacesModule {
            style,
            show_empty,
            dot_widths,
        },
        clock: ClockModule {
            style: cstyle,
            format,
            show_date,
            placeholder_clock,
        },
    })
}

fn resolve_launcher(theme_id: &str, l: Option<&RawLauncher>) -> anyhow::Result<Launcher> {
    let mode = match l.and_then(|l| l.mode.as_deref()) {
        None => LauncherMode::Overlay,
        Some("overlay") => LauncherMode::Overlay,
        Some("embedded") => LauncherMode::Embedded,
        Some(other) => {
            bail!("theme '{theme_id}': launcher.mode = \"{other}\" is not overlay|embedded")
        }
    };
    let width = l.and_then(|l| l.width).unwrap_or(540) as i32;
    let radius = l.and_then(|l| l.radius).unwrap_or(20) as i32;
    Ok(Launcher {
        mode,
        width,
        top: l
            .and_then(|l| l.top.clone())
            .unwrap_or_else(|| "16%".to_string()),
        radius,
        icon_px: l.and_then(|l| l.icon_px).unwrap_or(36) as i32,
        row_anim: l
            .and_then(|l| l.row_anim.clone())
            .unwrap_or_else(|| "flip".to_string()),
        rule: l
            .and_then(|l| l.rule.clone())
            .unwrap_or_else(|| "gradient".to_string()),
        footer: l
            .and_then(|l| l.footer.clone())
            .unwrap_or_else(|| "count_apps".to_string()),
        sections: l.and_then(|l| l.sections).unwrap_or(false),
        modes: l
            .and_then(|l| l.modes.clone())
            .unwrap_or_else(|| vec!["apps".to_string()]),
        // Default to the idle value — a theme that never sets these gets
        // "no change while searching", not a hardcoded widen/shrink it
        // never asked for (see the field docs on `Launcher`).
        search_width: l
            .and_then(|l| l.search_width)
            .map(|v| v as i32)
            .unwrap_or(width),
        search_radius: l
            .and_then(|l| l.search_radius)
            .map(|v| v as i32)
            .unwrap_or(radius),
        // Defaults below reproduce breadbox's pre-redesign hardcoded CSS
        // literals (`row { padding: 8px 12px; border-radius: 6px; }`,
        // `listbox { padding: 4px; }`, no icon swatch, a solid — not
        // alpha-blended — selection fill), so a theme that omits this whole
        // block of new keys (spotlight does; its capsule doesn't read them
        // at all) renders the same as before this change, not a jump to
        // either built-in's specific numbers.
        row_radius: l.and_then(|l| l.row_radius).unwrap_or(6) as i32,
        row_inset: l.and_then(|l| l.row_inset).unwrap_or(4) as i32,
        row_padding_v: l.and_then(|l| l.row_padding_v).unwrap_or(8) as i32,
        row_padding_h: l.and_then(|l| l.row_padding_h).unwrap_or(12) as i32,
        icon_radius: l.and_then(|l| l.icon_radius).unwrap_or(0) as i32,
        search_font_size: l.and_then(|l| l.search_font_size).unwrap_or(16) as i32,
        search_padding_v: l.and_then(|l| l.search_padding_v).unwrap_or(12) as i32,
        search_padding_h: l.and_then(|l| l.search_padding_h).unwrap_or(16) as i32,
        // 0.93 default, not breadbox's historical 0.60: a launcher panel needs
        // near-opacity to stay legible over a bright wallpaper.
        panel_alpha: l.and_then(|l| l.panel_alpha).unwrap_or(0.93),
        selection_alpha: l.and_then(|l| l.selection_alpha).unwrap_or(0.24),
    })
}

fn resolve_surfaces(
    theme_id: &str,
    raw: Option<&HashMap<String, RawSurface>>,
) -> anyhow::Result<BTreeMap<String, Surface>> {
    let mut out = BTreeMap::new();
    let Some(raw) = raw else { return Ok(out) };
    for (namespace, s) in raw {
        let anchor = match s.anchor.as_deref() {
            Some(a @ ("top_right" | "bottom_right" | "bottom_centre" | "fill")) => a.to_string(),
            Some(other) => bail!(
                "theme '{theme_id}': surfaces.{namespace}.anchor = \"{other}\" is not \
                 top_right|bottom_right|bottom_centre|fill (the only shapes breadbar's \
                 satellite windows implement — see breadbar/src/surface.rs)"
            ),
            None => bail!(
                "theme '{theme_id}': surfaces.{namespace} has no anchor set \
                 (expected one of top_right|bottom_centre|fill)"
            ),
        };
        let offset = match &s.offset {
            None => vec![],
            Some(RawOffset::Single(v)) => vec![*v],
            Some(RawOffset::Pair(v)) => v.to_vec(),
        };
        let width = match &s.width {
            None => SurfaceWidth::Auto,
            Some(RawSurfaceWidth::Named(n)) if n == "fill" => SurfaceWidth::Fill,
            Some(RawSurfaceWidth::Named(n)) if n == "auto" => SurfaceWidth::Auto,
            Some(RawSurfaceWidth::Named(other)) => bail!(
                "theme '{theme_id}': surfaces.{namespace}.width = \"{other}\" is not \"fill\" or \"auto\" \
                 (use a bare number for a fixed width)"
            ),
            Some(RawSurfaceWidth::Px(n)) => SurfaceWidth::Px(*n as i32),
        };
        let layer = match s.layer.as_deref() {
            None | Some("overlay") => "overlay".to_string(),
            Some("top") => "top".to_string(),
            Some(other) => bail!(
                "theme '{theme_id}': surfaces.{namespace}.layer = \"{other}\" is not top|overlay"
            ),
        };
        out.insert(
            namespace.clone(),
            Surface {
                anchor,
                offset,
                width,
                layer,
            },
        );
    }
    Ok(out)
}

fn resolve_compositor(raw: Option<&HashMap<String, RawLayerRule>>) -> BTreeMap<String, LayerRule> {
    let mut out = BTreeMap::new();
    let Some(raw) = raw else { return out };
    for (namespace, r) in raw {
        out.insert(
            namespace.clone(),
            LayerRule {
                blur: r.blur.unwrap_or(false),
                ignore_alpha: r.ignore_alpha,
                blur_popups: r.blur_popups.unwrap_or(false),
                animation: r.animation.clone(),
                no_anim: r.no_anim.unwrap_or(false),
            },
        );
    }
    out
}

impl RawManifest {
    /// Fill every default and validate every enum-ish string, producing a
    /// fully-resolved [`super::ShellTheme`]. `requested_id` is the id this
    /// manifest was looked up under (used as the id/name fallback when the
    /// TOML omits `id`/`name`); `css_template` and `extra_css` are threaded
    /// in by the discovery/extends logic in `mod.rs` since neither is a
    /// plain TOML field (extra_css is *read from* a TOML field, `css`, but
    /// resolving that path against the theme's own directory happens in the
    /// caller, which is the only place that still has the directory handy).
    pub(super) fn resolve(
        &self,
        requested_id: &str,
        css_template: String,
        extra_css: Option<String>,
    ) -> anyhow::Result<super::ShellTheme> {
        let id = self.id.clone().unwrap_or_else(|| requested_id.to_string());
        let name = self.name.clone().unwrap_or_else(|| id.clone());

        let tokens = resolve_tokens(&id, &self.tokens)?;

        let window = match self.bar.as_ref().and_then(|b| b.window.as_ref()) {
            Some(w) => resolve_window(&id, w)?,
            None => WindowSpec::default(),
        };

        let slots = self
            .bar
            .as_ref()
            .and_then(|b| b.slots.as_ref())
            .map(|s| Slots {
                left: s.left.clone(),
                centre: s.centre.clone(),
                right: s.right.clone(),
                drawer: s.drawer.clone(),
            })
            .unwrap_or_default();

        let modules = resolve_modules(&id, self.modules.as_ref())?;
        let launcher = resolve_launcher(&id, self.launcher.as_ref())?;
        let surfaces = resolve_surfaces(&id, self.surfaces.as_ref())?;
        let compositor = resolve_compositor(self.compositor.as_ref());

        // Render the theme's CSS now, once: strip the template's authoring
        // comments (a `{light}` written in prose must not be substituted, and
        // a `{name}` in prose must not trip the unresolved-ref check below),
        // substitute `[tokens]` *and* the `shell::style` derivations, then
        // append the optional `extra.css` overlay. A `{name}` that survives
        // matched none of those — a typo, and a hard error rather than a
        // literal `{radus_bar}` in the output.
        let mut pairs = tokens.subst_pairs();
        pairs.extend(super::style::subst_pairs(&tokens, &modules, &launcher));

        let mut resolved_css = substitute_pairs(&strip_css_comments(&css_template), pairs.clone());
        if let Some(extra) = &extra_css {
            let extra = substitute_pairs(&strip_css_comments(extra), pairs.clone());
            if !resolved_css.is_empty() && !resolved_css.ends_with('\n') {
                resolved_css.push('\n');
            }
            resolved_css.push_str(&extra);
        }
        let missing = unresolved_refs(&resolved_css);
        if !missing.is_empty() {
            bail!(
                "theme '{id}': CSS template references unknown name(s): {} \
                 (not a [tokens] field, not a shell::style derivation, and not \
                 defined in this theme's extra.css)",
                missing.join(", ")
            );
        }

        Ok(super::ShellTheme {
            name,
            id,
            tokens,
            window,
            slots,
            modules,
            launcher,
            surfaces,
            compositor,
            resolved_css,
        })
    }
}

/// Deep-merge `over` onto `base`: tables merge key-by-key recursively.
///
/// Arrays replace wholesale **unless** the `over` array contains the splice
/// marker `"+"`, in which case each `"+"` is replaced by the base array's
/// elements in place — so a child theme can add one entry to an inherited
/// slot list without restating it:
///
/// ```toml
/// extends = "liquid-motion"
/// [bar.slots]
/// right = ["+", "widget:my-thing"]   # inherited right slot, then my-thing
/// ```
///
/// This is `extends`'s one-level merge (plan §4/§11): `mod.rs` calls it once
/// per `load_named`, with the base's own `extends` key already stripped so a
/// chain can't go deeper than one level.
pub(super) fn merge_values(base: toml::Value, over: toml::Value) -> toml::Value {
    match (base, over) {
        (toml::Value::Table(mut base_t), toml::Value::Table(over_t)) => {
            for (k, v) in over_t {
                let merged = match base_t.remove(&k) {
                    Some(existing) => merge_values(existing, v),
                    None => v,
                };
                base_t.insert(k, merged);
            }
            toml::Value::Table(base_t)
        }
        (toml::Value::Array(base_a), toml::Value::Array(over_a))
            if over_a.iter().any(|v| v.as_str() == Some("+")) =>
        {
            let mut out = Vec::with_capacity(over_a.len() + base_a.len());
            for v in over_a {
                if v.as_str() == Some("+") {
                    out.extend(base_a.iter().cloned());
                } else {
                    out.push(v);
                }
            }
            toml::Value::Array(out)
        }
        (_, over) => over,
    }
}
