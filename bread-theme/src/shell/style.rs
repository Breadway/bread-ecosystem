//! Derived style values — the branching that used to live in
//! `breadbar::theme::load_css` (one function rendering all four themes with
//! `if light` / `if segmented` / per-`WorkspaceStyle` arms).
//!
//! These are computed from the *declarative* inputs a theme already sets —
//! `tokens.light`, `tokens.bar_border`, `modules.workspaces.style`,
//! `launcher.search_radius` — and handed to [`crate::shell::ShellTheme::css`]
//! as extra `{name}` substitution pairs on top of [`Tokens::subst_pairs`], so
//! the shared `assets/shell/base.css` template needs no conditional syntax:
//! every theme renders from the one template, its differences falling out of
//! these resolved values.

use super::types::{fmt_f64, BarBorder, Launcher, Modules, Tokens, WorkspaceStyle};

/// One chip-highlight height per bar, keyed on the workspace style (Trail's
/// pills are taller than Pill/Dots). Matches `breadbar`'s historical
/// `approved_chip_height`; that helper can be dropped once `breadbar` reads
/// this path for its `set_size_request` minimum too.
pub fn chip_height(style: WorkspaceStyle) -> i64 {
    match style {
        WorkspaceStyle::Trail => 26,
        WorkspaceStyle::Pill | WorkspaceStyle::Dots => 22,
    }
}

/// The `{name}` pairs the base template needs beyond the plain `[tokens]`.
/// Every value is fully resolved to literal CSS (only `@palette` names left);
/// none references another derived name, so substitution order doesn't matter.
pub(super) fn subst_pairs(t: &Tokens, m: &Modules, l: &Launcher) -> Vec<(String, String)> {
    let light = t.light;
    // `@bg` is a FIXED dark hex and `@on-bg` its computed near-white ink
    // (never pywal-derived). A light theme swaps which plays surface vs ink;
    // this works *only* because the two are pinned opposite by construction.
    let (panel, ink) = if light {
        ("@on-bg", "@bg")
    } else {
        ("@bg", "@on-bg")
    };
    let bg_alpha = fmt_f64(t.bg_alpha);

    // Card/panel fills: literal 0.70/0.72 glass for a dark theme; the theme's
    // own near-opaque `bg_alpha` for a light "paper" theme.
    let card_alpha = fmt_f64(if light { t.bg_alpha } else { 0.70 });
    let panel_surface_alpha = fmt_f64(if light { t.bg_alpha } else { 0.72 });
    // Progress-trough unfilled track: an accent tint reads fine on a dark
    // pill but vanishes on a near-white one, so a light theme uses a neutral
    // faint-ink track instead.
    let trough_bg = if light {
        format!("alpha({ink}, 0.14)")
    } else {
        "alpha(@accent, 0.25)".to_string()
    };
    let radius_search = format!("{}px", l.search_radius);

    let flush = t.bar_border == BarBorder::Bottom;
    let segmented = t.bar_border == BarBorder::Segmented;
    let radius_bar = format!("{}px", t.radius_bar);
    let window_chrome = if segmented {
        "background-color: transparent; border: none; box-shadow: none;".to_string()
    } else if flush {
        format!(
            "background-color: alpha({panel}, {bg_alpha}); border: none; \
             border-bottom: 1px solid alpha({ink}, 0.07);"
        )
    } else {
        format!(
            "background-color: alpha({panel}, {bg_alpha}); border: 1px solid alpha({ink}, 0.08);"
        )
    };
    let bar_radius = if segmented {
        "0px".to_string()
    } else {
        radius_bar.clone()
    };
    let centerbox_padding = if flush || segmented {
        "0 14px"
    } else {
        "0 8px 0 6px"
    }
    .to_string();
    let segment_css = if segmented {
        format!(
            ".bar-segment {{ background-color: alpha({panel}, {bg_alpha}); \
             border: 1px solid alpha({ink}, 0.10); border-radius: {radius_bar}; \
             box-shadow: 0 2px 10px alpha({ink}, 0.13); }}"
        )
    } else {
        String::new()
    };

    let style = m.workspaces.style;
    let radius_sm = format!("{}px", t.radius_sm);
    let radius_pill = format!("{}px", t.radius_pill);
    let chip_radius = match style {
        WorkspaceStyle::Dots => radius_pill.clone(),
        _ => radius_sm.clone(),
    };
    let chip_height_px = format!("{}px", chip_height(style));
    let settle = &t.spring_settle;
    let from = &t.accent_from;
    let to = &t.accent_to;

    let workspace_css = match style {
        WorkspaceStyle::Trail => format!(
            ".workspace-trail {{ background-image: linear-gradient(90deg, @{from}, @{to}); \
                 background-color: @{from}; border-radius: {radius_sm}; }} \
             .workspace-btn {{ background: transparent; opacity: 0.36; color: {ink}; \
                 border-radius: {radius_sm}; border: none; outline: none; box-shadow: none; \
                 min-width: 28px; min-height: {chip_height_px}; margin: 0; padding: 0 7px; \
                 font-size: 22px; font-weight: bold; \
                 transition: opacity 0.22s {settle}, background-color 0.22s {settle}; }} \
             .workspace-btn:hover {{ opacity: 0.85; background: alpha({ink}, 0.08); }} \
             .workspace-btn.occupied {{ opacity: 0.78; }} \
             .workspace-btn.active {{ background: transparent; color: @on-accent; opacity: 1; }} \
             .workspace-btn.active:hover {{ background: transparent; }} \
             .workspace-btn.ws-in {{ animation: row-in 0.32s {settle} both; }}"
        ),
        WorkspaceStyle::Pill => format!(
            ".workspace-btn {{ background: transparent; opacity: 1; color: alpha({ink}, 0.4); \
                 border-radius: {radius_sm}; border: none; outline: none; box-shadow: none; \
                 min-width: 22px; min-height: {chip_height_px}; margin: 0; padding: 0 6px; \
                 font-size: 12px; font-weight: 600; \
                 transition: background-color 0.22s {settle}, color 0.22s {settle}, \
                     opacity 0.22s {settle}; }} \
             .workspace-btn:hover {{ background: alpha({ink}, 0.08); }} \
             .workspace-btn.occupied {{ color: alpha({ink}, 0.8); }} \
             .workspace-btn:not(.occupied):not(.active) {{ opacity: 0.35; }} \
             .workspace-btn.active {{ background: @{from}; color: @on-accent; opacity: 1; }} \
             .workspace-btn.active:hover {{ background: @{from}; }} \
             .workspace-btn.ws-in {{ animation: row-in 0.32s {settle} both; }}"
        ),
        WorkspaceStyle::Dots => format!(
            ".workspace-dot {{ background-color: alpha({ink}, 0.35); color: transparent; \
                 border-radius: {radius_pill}; border: none; outline: none; box-shadow: none; \
                 min-height: 9px; margin: 0; padding: 0; \
                 transition: background-color 0.25s {settle}, opacity 0.25s {settle}; }} \
             .workspace-dot:hover {{ background-color: alpha({ink}, 0.55); }} \
             .workspace-dot:not(.occupied):not(.active) {{ opacity: 0.35; }} \
             .workspace-dot.active {{ background-color: @{from}; opacity: 1; }} \
             .workspace-dot.active:hover {{ background-color: @{from}; }}"
        ),
    };

    // breadbox's overlay-launcher geometry — every value straight off
    // `[launcher]`, the `{launcher_footer_case}` fragment its one derivation
    // (`sections` also drives the uppercase footer, as in the demos).
    let footer_case = if l.sections {
        "text-transform: uppercase; letter-spacing: 0.12em;"
    } else {
        ""
    };

    vec![
        ("panel".into(), panel.into()),
        ("ink".into(), ink.into()),
        ("card_alpha".into(), card_alpha),
        ("panel_surface_alpha".into(), panel_surface_alpha),
        ("trough_bg".into(), trough_bg),
        ("radius_search".into(), radius_search),
        ("window_chrome".into(), window_chrome),
        ("bar_radius".into(), bar_radius),
        ("centerbox_padding".into(), centerbox_padding),
        ("segment_css".into(), segment_css),
        ("chip_radius".into(), chip_radius),
        ("chip_height_px".into(), chip_height_px),
        ("workspace_css".into(), workspace_css),
        ("launcher_radius".into(), l.radius.to_string()),
        ("launcher_panel_alpha".into(), fmt_f64(l.panel_alpha)),
        (
            "launcher_selection_alpha".into(),
            fmt_f64(l.selection_alpha),
        ),
        ("launcher_row_pv".into(), l.row_padding_v.to_string()),
        ("launcher_row_ph".into(), l.row_padding_h.to_string()),
        ("launcher_row_inset".into(), l.row_inset.to_string()),
        ("launcher_row_radius".into(), l.row_radius.to_string()),
        ("launcher_icon_radius".into(), l.icon_radius.to_string()),
        ("launcher_search_pv".into(), l.search_padding_v.to_string()),
        ("launcher_search_ph".into(), l.search_padding_h.to_string()),
        ("launcher_search_fs".into(), l.search_font_size.to_string()),
        ("launcher_footer_case".into(), footer_case.into()),
    ]
}
