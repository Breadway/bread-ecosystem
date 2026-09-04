//! The compiled-in themes (plan §11 phase 1/5: "**One** built-in manifest
//! (`liquid-motion`) describing the bar as it exists today", extended in
//! Phase 5 with `glass-workbench`, demo 02). Every file here is plain data,
//! not Rust — each `theme.toml` is the manifest text a user override would
//! otherwise supply, and each `<id>.css` is the CSS template `ShellTheme::css`
//! substitutes tokens into (see that method's doc comment for why this
//! template is a representative subset of `breadbar::theme::load_css`'s
//! full stylesheet rather than a byte-for-byte copy of it).
//!
//! All are read with `include_str!` so a broken build can't ship without
//! them, and so [`super`] never touches the filesystem for a builtin — it
//! must work identically whether or not `$XDG_CONFIG_HOME` exists at all.

/// Every built-in theme renders from this one shared template. The per-theme
/// differences (light vs dark direction, island vs flush vs segmented window,
/// Trail vs Pill vs Dots workspaces) fall out of `[tokens]` +
/// `shell::style::subst_pairs`, so there is no per-theme CSS file and no
/// conditional syntax in the template. A user theme with a genuine one-off
/// still layers its own `extra.css` on top.
pub(super) const BASE_CSS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/shell/base.css"
));

pub const LIQUID_MOTION_ID: &str = "liquid-motion";

const LIQUID_MOTION_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/shell/liquid-motion/theme.toml"
));

pub const GLASS_WORKBENCH_ID: &str = "glass-workbench";

const GLASS_WORKBENCH_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/shell/glass-workbench/theme.toml"
));

pub const SPOTLIGHT_ID: &str = "spotlight";

const SPOTLIGHT_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/shell/spotlight/theme.toml"
));

pub const DAYLIGHT_ID: &str = "daylight";

const DAYLIGHT_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/shell/daylight/theme.toml"
));

pub const LOAF_ID: &str = "loaf";

const LOAF_TOML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/shell/loaf/theme.toml"
));

/// One compiled-in theme's identity plus its two `include_str!`ed assets.
/// `id`/`name` are also duplicated inside `toml`'s own `id =`/`name =`
/// fields — kept here too so [`all`]/[`find`] can list/look up a builtin
/// without parsing TOML first (`super::list`'s builtin fallback entry, and
/// `super::find_source`'s existence check, both run before any manifest
/// parsing happens).
pub struct BuiltinTheme {
    pub id: &'static str,
    pub name: &'static str,
    pub toml: &'static str,
    pub css: &'static str,
}

/// Every compiled-in theme, in the order [`super::list`] should present
/// them. Adding a third builtin is one entry here plus its two asset files
/// — nothing else in `mod.rs` names a specific builtin id except the always-
/// -safe fallback ([`LIQUID_MOTION_ID`], deliberately still hardcoded at
/// its one call site in `super::resolve_builtin` — see that function's doc
/// comment for why that one reference must NOT become "whichever builtin is
/// listed first").
pub const ALL: &[BuiltinTheme] = &[
    BuiltinTheme {
        id: LIQUID_MOTION_ID,
        name: "Liquid Motion",
        toml: LIQUID_MOTION_TOML,
        css: BASE_CSS,
    },
    BuiltinTheme {
        id: GLASS_WORKBENCH_ID,
        name: "Glass Workbench",
        toml: GLASS_WORKBENCH_TOML,
        css: BASE_CSS,
    },
    BuiltinTheme {
        id: SPOTLIGHT_ID,
        name: "Spotlight",
        toml: SPOTLIGHT_TOML,
        css: BASE_CSS,
    },
    BuiltinTheme {
        id: DAYLIGHT_ID,
        name: "Daylight",
        toml: DAYLIGHT_TOML,
        css: BASE_CSS,
    },
    BuiltinTheme {
        id: LOAF_ID,
        name: "Loaf",
        toml: LOAF_TOML,
        css: BASE_CSS,
    },
];

/// Looks up a compiled-in theme by id — `None` means "not a builtin",
/// exactly like a miss in the user/system theme directories.
pub fn find(id: &str) -> Option<&'static BuiltinTheme> {
    ALL.iter().find(|t| t.id == id)
}
