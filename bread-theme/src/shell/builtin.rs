//! The one compiled-in theme (plan §11 phase 1: "**One** built-in manifest
//! (`liquid-motion`) describing the bar as it exists today"). Both files are
//! plain data, not Rust — `theme.toml` is the manifest text a user override
//! would otherwise supply, and `liquid-motion.css` is the CSS template
//! `ShellTheme::css` substitutes tokens into (see that method's doc comment
//! for why this template is a representative subset of `breadbar::theme::
//! load_css`'s full stylesheet rather than a byte-for-byte copy of it).
//!
//! Both are read with `include_str!` so a broken build can't ship without
//! them, and so [`super::builtin`] never touches the filesystem — it must
//! work identically whether or not `$XDG_CONFIG_HOME` exists at all.

pub const LIQUID_MOTION_ID: &str = "liquid-motion";

pub const LIQUID_MOTION_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/shell/liquid-motion/theme.toml"
));

pub const LIQUID_MOTION_CSS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/shell/liquid-motion/liquid-motion.css"
));
