//! A machine-readable description of the `theme.toml` schema — the token
//! fields with their types and defaults, the closed enum vocabularies, and
//! the currently-known slot module names.
//!
//! This is what a theme editor (bos-settings, plan §5) needs to build its
//! controls: not a full JSON-Schema validation document, but "here are the
//! knobs, their kinds, and their allowed values". Emitted by
//! `bread-theme describe`.

use serde_json::{json, Value};

use super::types::Tokens;

/// The full schema description as a JSON object.
pub fn describe() -> Value {
    let d = Tokens::default();
    json!({
        "tokens": {
            "radius_bar":      { "type": "int",   "default": d.radius_bar,  "unit": "px" },
            "radius_card":     { "type": "int",   "default": d.radius_card, "unit": "px" },
            "radius_sm":       { "type": "int",   "default": d.radius_sm,   "unit": "px" },
            "radius_pill":     { "type": "int",   "default": d.radius_pill, "unit": "px" },
            "pad":             { "type": "int",   "default": d.pad,         "unit": "px" },
            "chip_height":     { "type": "int",   "default": d.chip_height, "unit": "px" },
            "icon_px":         { "type": "int",   "default": d.icon_px,     "unit": "px" },
            "font_size_base":  { "type": "int",   "default": d.font_size_base, "unit": "px" },
            "bg_alpha":        { "type": "float", "default": d.bg_alpha, "range": [0.0, 1.0] },
            "spring":          { "type": "string", "default": d.spring, "hint": "cubic-bezier(...)" },
            "spring_settle":   { "type": "string", "default": d.spring_settle, "hint": "cubic-bezier(...)" },
            "font_family":     { "type": "string", "default": d.font_family },
            "font_fallback":   { "type": "string", "default": d.font_fallback },
            "accent_from":     { "type": "palette", "default": d.accent_from, "hint": "a palette slot name" },
            "accent_to":       { "type": "palette", "default": "= accent_from" },
            "accent2":         { "type": "palette", "default": "= accent_from" },
            "light":           { "type": "bool",  "default": d.light },
            "bar_border":      { "type": "enum",  "default": d.bar_border.as_str(),
                                 "values": ["full", "bottom", "segmented"] },
        },
        "palette_slots": ["bg", "fg", "surface", "overlay", "accent",
                          "red", "green", "yellow", "blue", "pink", "teal",
                          "on-bg", "on-surface", "on-accent", "on-red", "on-overlay"],
        "bar": {
            "window": {
                "anchors":   { "type": "list<enum>", "values": ["top", "bottom", "left", "right"] },
                "width":     { "type": "\"fill\" | int(px)" },
                "height":    { "type": "int", "unit": "px" },
                "margin":    { "type": "{ top, left, right, bottom }", "unit": "px" },
                "exclusive": { "type": "\"auto\" | \"none\" | int(px)" },
                "keyboard":  { "type": "enum", "values": ["none", "on_demand", "exclusive"] },
                "layer":     { "type": "enum", "values": ["top", "overlay"] },
            },
            "slots": {
                "keys": ["left", "centre", "right", "drawer"],
                "entry": "a module name, `widget:<key>`, or `\"+\"` (only when the theme `extends` another)",
                "modules": super::modules::all(),
            },
        },
        "modules": {
            "workspaces": {
                "style":      { "type": "enum", "values": ["trail", "pill", "dots"] },
                "show_empty": { "type": "bool" },
                "dot_widths": { "type": "[int; 4]", "note": "style = dots only" },
            },
            "clock": {
                "style":            { "type": "enum", "values": ["flip", "plain", "none"] },
                "format":           { "type": "string", "note": "style = plain only" },
                "show_date":        { "type": "bool",   "note": "style = plain only" },
                "placeholder_clock":{ "type": "bool",   "note": "style = none only" },
            },
        },
        "launcher": {
            "mode":  { "type": "enum", "values": ["overlay", "embedded"] },
            "note":  "geometry keys (radius, row_radius, panel_alpha, …) are CSS — see the demos",
        },
        "surfaces": {
            "key":    "a layer-shell namespace (breadbar-notif, breadbar-osd, …)",
            "anchor": { "type": "enum", "values": ["top_right", "bottom_right", "bottom_centre", "fill"] },
            "width":  { "type": "\"fill\" | \"auto\" | int(px)" },
            "layer":  { "type": "enum", "values": ["top", "overlay"] },
        },
        "compositor": {
            "key":    "a layer-shell namespace",
            "fields": ["blur", "ignore_alpha", "blur_popups", "animation", "no_anim"],
        },
        "css": "optional path to an extra.css overlay, resolved against the theme dir",
    })
}

/// [`describe`] as pretty-printed JSON.
pub fn describe_json() -> String {
    serde_json::to_string_pretty(&describe()).expect("schema description is plain JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_is_valid_json_with_the_key_sections() {
        let v = describe();
        for k in [
            "tokens",
            "bar",
            "modules",
            "launcher",
            "surfaces",
            "palette_slots",
        ] {
            assert!(v.get(k).is_some(), "missing section {k}");
        }
        // A token default round-trips.
        assert_eq!(v["tokens"]["bar_border"]["default"], "full");
        // The slot module list includes a builtin.
        let mods = v["bar"]["slots"]["modules"].as_array().unwrap();
        assert!(mods.iter().any(|m| m == "workspaces"));
    }
}
