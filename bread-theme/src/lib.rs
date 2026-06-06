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
