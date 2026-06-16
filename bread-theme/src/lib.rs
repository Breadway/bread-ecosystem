pub mod palette;
#[cfg(feature = "gtk")]
pub mod gtk;

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

/// Emit the `@define-color` block that all bread apps use.
/// Apps append their own rules below this; user CSS goes on top.
pub fn css_vars(p: &Palette) -> String {
    format!(
        "@define-color bg {bg};\n\
         @define-color fg {fg};\n\
         @define-color surface {c0};\n\
         @define-color red {c1};\n\
         @define-color green {c2};\n\
         @define-color yellow {c3};\n\
         @define-color blue {c4};\n\
         @define-color pink {c5};\n\
         @define-color teal {c6};\n\
         @define-color overlay {c7};\n\
         * {{ font-family: '{font}'; font-size: {size}px; }}\n",
        bg = p.background,
        fg = p.foreground,
        c0 = p.color0,
        c1 = p.color1,
        c2 = p.color2,
        c3 = p.color3,
        c4 = p.color4,
        c5 = p.color5,
        c6 = p.color6,
        c7 = p.color7,
        font = tokens::FONT_FAMILY,
        size = tokens::FONT_SIZE_BASE,
    )
}

/// Canonical `@define-color` block: the single naming all bread apps share.
/// `surface` = color0 (darkest surface), `overlay` = color7 (muted), and
/// `accent` = color4. Apps must use these names, not raw palette slots, so the
/// whole ecosystem recolours together.
fn define_colors(p: &Palette) -> String {
    format!(
        "@define-color bg {bg};\n\
         @define-color fg {fg};\n\
         @define-color surface {c0};\n\
         @define-color overlay {c7};\n\
         @define-color accent {c4};\n\
         @define-color red {c1};\n\
         @define-color green {c2};\n\
         @define-color yellow {c3};\n\
         @define-color blue {c4};\n\
         @define-color pink {c5};\n\
         @define-color teal {c6};\n",
        bg = p.background, fg = p.foreground,
        c0 = p.color0, c1 = p.color1, c2 = p.color2, c3 = p.color3,
        c4 = p.color4, c5 = p.color5, c6 = p.color6, c7 = p.color7,
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
         window {{ background-color: @bg; color: @fg; }}\n\
         label {{ color: @fg; }}\n\
         .dim-label, .dim {{ color: @fg; opacity: 0.6; font-size: {sec}px; }}\n\
         .title {{ font-size: 1.4em; font-weight: bold; color: @fg; }}\n\
         .heading {{ font-weight: bold; color: @fg; opacity: 0.85; }}\n\
         .subtitle {{ color: @fg; opacity: 0.7; font-size: {sec}px; }}\n\
         button {{ background-color: @surface; color: @fg; border: none;\
             border-radius: {r1}px; padding: {sm}px {lg}px; }}\n\
         button:hover {{ background-color: alpha(@fg, 0.14); }}\n\
         button:active {{ background-color: alpha(@fg, 0.20); }}\n\
         button:disabled {{ opacity: 0.5; }}\n\
         button.flat {{ background-color: transparent; }}\n\
         button.suggested-action {{ background-color: @accent; color: @bg; }}\n\
         button.suggested-action:hover {{ background-color: alpha(@accent, 0.85); }}\n\
         button.destructive-action {{ background-color: @red; color: @bg; }}\n\
         button.destructive-action:hover {{ background-color: alpha(@red, 0.85); }}\n\
         entry, spinbutton {{ background-color: @surface; color: @fg;\
             border: 1px solid @overlay; border-radius: {r2}px;\
             padding: {xs}px {sm}px; caret-color: @fg; }}\n\
         entry:focus-within, spinbutton:focus-within {{ border-color: @accent; outline: none; }}\n\
         entry image, spinbutton button {{ color: @fg; }}\n\
         dropdown > button {{ background-color: @surface; border-radius: {r2}px; }}\n\
         popover > contents {{ background-color: @surface; color: @fg; border-radius: {r1}px; }}\n\
         switch {{ background-color: @overlay; border-radius: {pill}px; }}\n\
         switch:checked {{ background-color: @accent; }}\n\
         switch slider {{ background-color: @fg; border-radius: {pill}px; }}\n\
         list, listbox {{ background-color: transparent; }}\n\
         row {{ border-radius: {r2}px; }}\n\
         row:selected, list row:selected {{ background-color: @accent; color: @bg; }}\n\
         .sidebar {{ background-color: @surface; }}\n\
         .sidebar row {{ padding: {sm}px {md}px; color: @fg; }}\n\
         .sidebar row:selected {{ background-color: @accent; color: @bg; }}\n\
         .sidebar .section-header {{ padding: {md}px {md}px {xs}px {md}px;\
             font-size: {sec}px; font-weight: bold; color: @fg; opacity: 0.55; }}\n\
         .card {{ background-color: @surface; border-radius: {r1}px; padding: {md}px; }}\n\
         .chip, .pill {{ background-color: @overlay; color: @fg; border-radius: {pill}px;\
             padding: {xs}px {md}px; font-size: {sec}px; }}\n\
         .chip.active, .pill.active {{ background-color: @accent; color: @bg; }}\n\
         scrollbar {{ background-color: transparent; }}\n\
         scrollbar slider {{ background-color: alpha(@fg, 0.25); border-radius: {pill}px;\
             min-width: 6px; min-height: 6px; }}\n\
         scrollbar slider:hover {{ background-color: alpha(@fg, 0.45); }}\n\
         textview, .mono {{ font-family: monospace; }}\n\
         textview text {{ background-color: @surface; color: @fg; }}\n",
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
    fn stylesheet_defines_canonical_colors_and_components() {
        let css = stylesheet(&Palette::default());
        for name in &["bg", "fg", "surface", "overlay", "accent", "red", "blue"] {
            assert!(css.contains(&format!("@define-color {name} ")), "missing @define-color {name}");
        }
        // a representative spread of the shared component selectors
        for sel in &["button", "entry", "switch:checked", ".card", ".sidebar", "scrollbar slider", ".title"] {
            assert!(css.contains(sel), "stylesheet missing selector: {sel}");
        }
        assert!(css.contains("Varela Round"));
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
