pub mod palette;
#[cfg(feature = "gtk")]
pub mod gtk;
#[cfg(feature = "adw")]
pub mod adw;

pub use palette::{load_palette, Palette};

/// Design tokens from BREAD_DESIGN_SYSTEM.md.
pub mod tokens {
    pub const FONT_FAMILY: &str = "Varela Round, sans-serif";
    pub const FONT_SIZE_BASE: u8 = 14;
    pub const FONT_SIZE_SECONDARY: u8 = 12;

    // Spacing scale (px, 4px units)
    pub const SPACE_XS: u8 = 4;
    pub const SPACE_SM: u8 = 8;
    pub const SPACE_MD: u8 = 12;
    pub const SPACE_LG: u8 = 16;
    pub const SPACE_XL: u8 = 20;

    // Border radius
    pub const RADIUS_PRIMARY: u8 = 8;
    pub const RADIUS_SECONDARY: u8 = 6;
    pub const RADIUS_TERTIARY: u8 = 4;
    pub const RADIUS_PILL: u16 = 999;
}

/// Emit the `@define-color` block that all bread apps use, plus the shared
/// font rule.
///
/// Kept for API compatibility with older callers that only want the color
/// variables (not the full [`stylesheet`] component rules). It used to carry
/// its own hand-written `@define-color` block that predated the `accent` and
/// computed-ink (`on-*`) colors — that duplication is exactly what let it
/// drift out of sync and reintroduce the illegible-text bug (light pywal
/// colors + no computed ink meant white-on-white / black-on-black text
/// wherever a caller's own CSS referenced `@on-surface`, `@on-accent`, etc.,
/// since those names simply didn't exist in this block). It now delegates
/// to the same [`define_colors`] the full stylesheet uses, so there is only
/// one color-block implementation and it cannot drift again.
pub fn css_vars(p: &Palette) -> String {
    format!(
        "{vars}* {{ font-family: '{font}'; font-size: {size}px; }}\n",
        vars = define_colors(p),
        font = tokens::FONT_FAMILY,
        size = tokens::FONT_SIZE_BASE,
    )
}

/// Relative luminance (WCAG, sRGB) of a `#rrggbb` colour, 0.0 (black) – 1.0 (white).
pub fn luminance(hex: &str) -> f32 {
    let h = hex.trim_start_matches('#');
    let lin = |i: usize| -> f32 {
        let c = u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0) as f32 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(0) + 0.7152 * lin(2) + 0.0722 * lin(4)
}

/// Pick a legible ink (near-black or near-white) for text drawn on `hex`.
/// 0.179 is the WCAG crossover where contrast against black equals contrast
/// against white — so whichever side we pick always wins. This is what keeps
/// text readable no matter how light or dark pywal makes a given palette slot,
/// without altering the palette colours themselves.
pub fn ink_on(hex: &str) -> &'static str {
    if luminance(hex) > 0.179 { "#11111b" } else { "#f5f5f5" }
}

/// Canonical (name, value) list: the single naming all bread apps share.
/// `surface` = color0 (darkest surface), `overlay` = color7 (muted), and
/// `accent` = color4. Apps must use these names, not raw palette slots, so the
/// whole ecosystem recolours together.
///
/// The `on-*` colours are computed ink (black/white) guaranteed to be legible on
/// the matching background — use `on-surface` for text on a `surface` panel,
/// `on-accent` on an `accent` button, etc. They exist because pywal can emit a
/// light value in any slot, and white text on a light surface disappears.
///
/// [`define_colors`] (GTK `@define-color`) and [`css_custom_properties`] (web
/// `:root { --name: ... }`) both format this same list rather than each
/// hand-writing their own — see `css_vars_and_stylesheet_agree_on_color_block`
/// and `css_custom_properties_matches_define_colors_name_set` for the
/// regression tests this exists to satisfy.
fn color_pairs(p: &Palette) -> [(&'static str, String); 16] {
    [
        ("bg", p.background.clone()),
        ("fg", p.foreground.clone()),
        ("surface", p.color0.clone()),
        ("overlay", p.color7.clone()),
        ("accent", p.color4.clone()),
        ("red", p.color1.clone()),
        ("green", p.color2.clone()),
        ("yellow", p.color3.clone()),
        ("blue", p.color4.clone()),
        ("pink", p.color5.clone()),
        ("teal", p.color6.clone()),
        ("on-bg", ink_on(&p.background).to_string()),
        ("on-surface", ink_on(&p.color0).to_string()),
        ("on-accent", ink_on(&p.color4).to_string()),
        ("on-red", ink_on(&p.color1).to_string()),
        ("on-overlay", ink_on(&p.color7).to_string()),
    ]
}

/// GTK `@define-color` block built from [`color_pairs`].
fn define_colors(p: &Palette) -> String {
    color_pairs(p)
        .iter()
        .map(|(name, value)| format!("@define-color {name} {value};\n"))
        .collect()
}

/// CSS custom-properties block (`:root { --bg: ...; --on-accent: ...; }`) for
/// web frontends (Tauri), using the exact same names as [`define_colors`] so
/// the GTK and web outputs cannot drift apart independently — both are
/// generated from [`color_pairs`], not two hand-written copies.
pub fn css_custom_properties(p: &Palette) -> String {
    let vars: String = color_pairs(p)
        .iter()
        .map(|(name, value)| format!("  --{name}: {value};\n"))
        .collect();
    format!(":root {{\n{vars}}}\n")
}

/// CSS custom-properties for [`tokens`] (font, spacing, radii) — the web
/// counterpart to [`tokens`] being hand-read by GTK code, so a web frontend
/// isn't hand-copying the same numbers into a second source of truth.
pub fn css_tokens() -> String {
    use tokens::*;
    format!(
        ":root {{\n\
         \x20\x20--font-family: '{font}';\n\
         \x20\x20--font-size-base: {base}px;\n\
         \x20\x20--font-size-secondary: {sec}px;\n\
         \x20\x20--space-xs: {xs}px;\n\
         \x20\x20--space-sm: {sm}px;\n\
         \x20\x20--space-md: {md}px;\n\
         \x20\x20--space-lg: {lg}px;\n\
         \x20\x20--space-xl: {xl}px;\n\
         \x20\x20--radius-primary: {r1}px;\n\
         \x20\x20--radius-secondary: {r2}px;\n\
         \x20\x20--radius-tertiary: {r3}px;\n\
         \x20\x20--radius-pill: {pill}px;\n\
         }}\n",
        font = FONT_FAMILY, base = FONT_SIZE_BASE, sec = FONT_SIZE_SECONDARY,
        xs = SPACE_XS, sm = SPACE_SM, md = SPACE_MD, lg = SPACE_LG, xl = SPACE_XL,
        r1 = RADIUS_PRIMARY, r2 = RADIUS_SECONDARY, r3 = RADIUS_TERTIARY, pill = RADIUS_PILL,
    )
}

/// The full shared component stylesheet — the single source of truth for how
/// every bread GUI (bos-settings, breadbar, breadbox, breadpad, breadman) styles
/// common widgets. Apps load this, then append only their own *layout* rules.
///
/// Built entirely from the design tokens (font, spacing, radii) and the
/// `@define-color` palette, so changing the palette recolours every app.
pub fn stylesheet(p: &Palette) -> String {
    use tokens::*;
    format!(
        "{vars}\
         * {{ font-family: '{font}'; font-size: {base}px; }}\n\
         /* Colour is set on containers; labels inherit it, so text on any panel,\
            button, or accent is always the legible ink for that background. Bare\
            `label {{ color }}` is deliberately avoided — as a type selector it\
            would override a container's colour on its own child labels. */\n\
         window {{ background-color: @bg; color: @on-bg; }}\n\
         .dim-label, .dim {{ opacity: 0.6; font-size: {sec}px; }}\n\
         /* Named `.page-title`, not the more obvious `.title` - libadwaita's\
            own row/window-title widgets (AdwActionRow, AdwWindowTitle, GtkHeaderBar)\
            put a bare `title` CSS class on their internal label, so a generic\
            `.title` rule here would inflate every libadwaita row's title text\
            to 1.4em too (this is exactly what caused the settings screen's\
            ~24px row-title bug). Scoping the name avoids the collision instead\
            of trying to out-specificity a first-party GTK/libadwaita class. */\n\
         .page-title {{ font-size: 1.4em; font-weight: bold; }}\n\
         .heading {{ font-weight: bold; opacity: 0.85; }}\n\
         /* Same libadwaita-collision reasoning as `.page-title` above - a bare\
            `.subtitle` also matches libadwaita's internal row-subtitle labels.\
            Unused by any app today, but scoped so a future caller doesn't\
            reintroduce the fight. */\n\
         .page-subtitle {{ opacity: 0.7; font-size: {sec}px; }}\n\
         button {{ background-color: @surface; color: @on-surface; border: none;\
             border-radius: {r1}px; padding: {sm}px {lg}px; }}\n\
         button:hover {{ background-color: alpha(@on-surface, 0.14); }}\n\
         button:active {{ background-color: alpha(@on-surface, 0.20); }}\n\
         button:disabled {{ opacity: 0.5; }}\n\
         button.flat {{ background-color: transparent; color: @on-bg; }}\n\
         button.suggested-action {{ background-color: @accent; color: @on-accent; }}\n\
         button.suggested-action:hover {{ background-color: alpha(@accent, 0.85); }}\n\
         button.destructive-action {{ background-color: @red; color: @on-red; }}\n\
         button.destructive-action:hover {{ background-color: alpha(@red, 0.85); }}\n\
         entry, spinbutton {{ background-color: @surface; color: @on-surface;\
             border: 1px solid @overlay; border-radius: {r2}px;\
             padding: {xs}px {sm}px; caret-color: @on-surface; }}\n\
         entry:focus-within, spinbutton:focus-within {{ border-color: @accent; outline: none; }}\n\
         entry image, spinbutton button {{ color: @on-surface; }}\n\
         dropdown > button {{ background-color: @surface; color: @on-surface; border-radius: {r2}px; }}\n\
         popover > contents {{ background-color: @surface; color: @on-surface; border-radius: {r1}px; }}\n\
         switch {{ background-color: @overlay; border-radius: {pill}px; }}\n\
         switch:checked {{ background-color: @accent; }}\n\
         switch slider {{ background-color: @on-surface; border-radius: {pill}px; }}\n\
         /* GtkScale (sliders) render with GTK's own default accent (a fixed\
            blue, independent of the app's theme) unless styled explicitly —\
            every app with a volume/brightness slider was silently showing\
            that default instead of the palette's accent until this rule\
            existed. */\n\
         scale trough {{ background-color: @overlay; border-radius: {pill}px; min-height: 6px; }}\n\
         scale trough highlight {{ background-color: @accent; border-radius: {pill}px; min-height: 6px; }}\n\
         scale slider {{ background-color: @on-bg; border-radius: {pill}px; }}\n\
         list, listbox {{ background-color: transparent; }}\n\
         /* libadwaita's AdwPreferencesGroup wraps its rows in a GtkListBox\
            carrying the `boxed-list` class, expecting a surface fill + radius\
            to read as a card. The bare-type rule above (needed so plain\
            GTK4 sidebars/lists stay transparent) was overriding that with\
            equal specificity and no fill ever won, leaving preference groups\
            as a bare bordered table instead of a card. This is scoped to the\
            class only, so it doesn't touch any non-adw list. */\n\
         list.boxed-list, listbox.boxed-list {{ background-color: @surface; border-radius: {r1}px; }}\n\
         row {{ border-radius: {r2}px; }}\n\
         row:selected, list row:selected {{ background-color: @accent; color: @on-accent; }}\n\
         .sidebar {{ background-color: @surface; color: @on-surface; }}\n\
         .sidebar row {{ padding: {sm}px {md}px; }}\n\
         .sidebar row:selected {{ background-color: @accent; color: @on-accent; }}\n\
         .sidebar .section-header {{ padding: {md}px {md}px {xs}px {md}px;\
             font-size: {sec}px; font-weight: bold; opacity: 0.55; }}\n\
         .card {{ background-color: @surface; color: @on-surface; border-radius: {r1}px; padding: {md}px; }}\n\
         .chip, .pill {{ background-color: @overlay; color: @on-overlay; border-radius: {pill}px;\
             padding: {xs}px {md}px; font-size: {sec}px; }}\n\
         .chip.active, .pill.active {{ background-color: @accent; color: @on-accent; }}\n\
         scrollbar {{ background-color: transparent; }}\n\
         scrollbar slider {{ background-color: alpha(@on-bg, 0.25); border-radius: {pill}px;\
             min-width: 6px; min-height: 6px; }}\n\
         scrollbar slider:hover {{ background-color: alpha(@on-bg, 0.45); }}\n\
         textview, .mono {{ font-family: monospace; }}\n\
         textview text {{ background-color: @surface; color: @on-surface; }}\n",
        vars = define_colors(p),
        font = FONT_FAMILY,
        base = FONT_SIZE_BASE,
        sec = FONT_SIZE_SECONDARY,
        xs = SPACE_XS, sm = SPACE_SM, md = SPACE_MD, lg = SPACE_LG,
        r1 = RADIUS_PRIMARY, r2 = RADIUS_SECONDARY, pill = RADIUS_PILL,
    )
}

/// Render the shared stylesheet for the current (pywal) palette. Used by the
/// `bread-theme` generator and as the in-app fallback when the generated file
/// isn't present yet.
pub fn render() -> String {
    stylesheet(&load_palette())
}

/// Canonical path of the generated shared stylesheet. Apps load it; the
/// `bread-theme generate` CLI writes it. Per-session under `XDG_RUNTIME_DIR`,
/// falling back to the cache dir.
pub fn shared_css_path() -> std::path::PathBuf {
    if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
        if !rt.is_empty() {
            return std::path::PathBuf::from(rt).join("bread").join("theme.css");
        }
    }
    dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("bread")
        .join("theme.css")
}

/// Write the shared stylesheet to [`shared_css_path`] (atomic rename). Returns
/// the path written. Used by the `bread-theme` CLI.
pub fn write_shared_css() -> std::io::Result<std::path::PathBuf> {
    let path = shared_css_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("css.tmp");
    std::fs::write(&tmp, render())?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Convert a `#rrggbb` hex colour to `rgba(r, g, b, alpha)`.
pub fn hex_to_rgba(hex: &str, alpha: f32) -> String {
    let h = hex.trim_start_matches('#');
    let r = u8::from_str_radix(h.get(0..2).unwrap_or("00"), 16).unwrap_or(0);
    let g = u8::from_str_radix(h.get(2..4).unwrap_or("00"), 16).unwrap_or(0);
    let b = u8::from_str_radix(h.get(4..6).unwrap_or("00"), 16).unwrap_or(0);
    format!("rgba({r}, {g}, {b}, {alpha})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_vars_contains_all_define_color_names() {
        let css = css_vars(&Palette::default());
        for name in &["bg", "fg", "surface", "red", "green", "yellow", "blue", "pink", "teal", "overlay"] {
            assert!(css.contains(&format!("@define-color {name} ")), "missing @define-color {name}");
        }
    }

    #[test]
    fn css_vars_contains_font_rule() {
        let css = css_vars(&Palette::default());
        assert!(css.contains("Varela Round"));
        assert!(css.contains("14px"));
    }

    #[test]
    fn css_vars_includes_accent_and_computed_ink_colors() {
        // Regression test: css_vars() used to be a second, hand-written
        // @define-color block that predated `accent` and the computed `on-*`
        // ink colors. Any caller whose own CSS referenced `@on-surface` /
        // `@on-accent` etc. against that older block would hit an undefined
        // color name — the illegible-text bug. css_vars() must now emit
        // exactly the same color set as the full stylesheet.
        let css = css_vars(&Palette::default());
        for name in &["accent", "on-bg", "on-surface", "on-accent", "on-red", "on-overlay"] {
            assert!(css.contains(&format!("@define-color {name} ")), "missing @define-color {name}");
        }
    }

    #[test]
    fn css_vars_and_stylesheet_agree_on_color_block() {
        // Both must derive their color variables from the same
        // `define_colors` implementation, so they can't drift apart again.
        let p = Palette::default();
        let vars = css_vars(&p);
        let sheet = stylesheet(&p);
        for name in &["bg", "fg", "surface", "overlay", "accent", "on-bg", "on-surface", "on-accent"] {
            let needle = format!("@define-color {name} ");
            assert!(vars.contains(&needle) && sheet.contains(&needle));
        }
    }

    #[test]
    fn stylesheet_defines_canonical_colors_and_components() {
        let css = stylesheet(&Palette::default());
        for name in &["bg", "fg", "surface", "overlay", "accent", "red", "blue"] {
            assert!(css.contains(&format!("@define-color {name} ")), "missing @define-color {name}");
        }
        // a representative spread of the shared component selectors
        for sel in &["button", "entry", "switch:checked", ".card", ".sidebar", "scrollbar slider", ".page-title"] {
            assert!(css.contains(sel), "stylesheet missing selector: {sel}");
        }
        assert!(css.contains("Varela Round"));
    }

    #[test]
    fn css_custom_properties_matches_define_colors_name_set() {
        // Both must derive from the same color_pairs() list, so the web
        // output can't drift from the GTK one the way css_vars/stylesheet
        // used to (see css_vars_and_stylesheet_agree_on_color_block above).
        let p = Palette::default();
        let gtk = define_colors(&p);
        let web = css_custom_properties(&p);
        for (name, _) in color_pairs(&p) {
            assert!(gtk.contains(&format!("@define-color {name} ")), "gtk missing {name}");
            assert!(web.contains(&format!("--{name}: ")), "web missing {name}");
        }
    }

    #[test]
    fn css_custom_properties_is_valid_root_block() {
        let p = Palette::default();
        let css = css_custom_properties(&p);
        assert!(css.starts_with(":root {\n"));
        assert!(css.trim_end().ends_with('}'));
        assert!(css.contains(&format!("--accent: {};", p.color4)));
    }

    #[test]
    fn css_tokens_contains_font_and_spacing_vars() {
        let css = css_tokens();
        assert!(css.contains("--font-family: 'Varela Round, sans-serif';"));
        assert!(css.contains("--font-size-base: 14px;"));
        assert!(css.contains("--space-md: 12px;"));
        assert!(css.contains("--radius-pill: 999px;"));
    }

    #[test]
    fn luminance_black_and_white_are_extremes() {
        assert!(luminance("#000000") < 0.01);
        assert!(luminance("#ffffff") > 0.99);
    }

    #[test]
    fn ink_on_picks_dark_text_for_light_backgrounds() {
        // Light pywal slots (the case that made white text vanish) get dark ink.
        assert_eq!(ink_on("#ffffff"), "#11111b");
        assert_eq!(ink_on("#f9e2af"), "#11111b"); // pale yellow
        assert_eq!(ink_on("#a6e3a1"), "#11111b"); // pale green
    }

    #[test]
    fn ink_on_picks_light_text_for_dark_backgrounds() {
        assert_eq!(ink_on("#000000"), "#f5f5f5");
        assert_eq!(ink_on("#1e1e2e"), "#f5f5f5"); // catppuccin base
    }

    #[test]
    fn stylesheet_defines_on_colors() {
        let css = stylesheet(&Palette::default());
        for name in &["on-bg", "on-surface", "on-accent", "on-red", "on-overlay"] {
            assert!(css.contains(&format!("@define-color {name} ")), "missing @define-color {name}");
        }
    }

    #[test]
    fn stylesheet_has_no_blanket_label_color_rule() {
        // A bare `label { color: ... }` would override container colours on child
        // labels — the bug that made coloured-background text illegible.
        let css = stylesheet(&Palette::default());
        assert!(!css.contains("label { color:"), "blanket label colour rule reintroduced");
    }

    #[test]
    fn shared_css_path_uses_runtime_dir() {
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1234");
        assert_eq!(shared_css_path(), std::path::PathBuf::from("/run/user/1234/bread/theme.css"));
    }

    #[test]
    fn render_is_nonempty_css() {
        assert!(render().contains("@define-color bg "));
    }

    #[test]
    fn hex_to_rgba_known_value() {
        assert_eq!(hex_to_rgba("#1e1e2e", 1.0), "rgba(30, 30, 46, 1)");
    }

    #[test]
    fn hex_to_rgba_strips_hash() {
        let a = hex_to_rgba("#ffffff", 0.5);
        let b = hex_to_rgba("ffffff", 0.5);
        assert_eq!(a, b);
    }
}
