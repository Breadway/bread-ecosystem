use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// BOS's fixed dark theme: background, surface, overlay, and foreground never
/// come from pywal. Only the accent slots (color1-6) track the wallpaper.
/// Without this, a light or muddy-toned wallpaper (a beige bread photo, a
/// snapshot, a bright desktop screenshot) makes pywal hand back a light or
/// off-hue background, and every bread GUI's panels inherit it: the app
/// stops looking like a dark BOS tool and starts looking like whatever colour
/// the wallpaper happened to be.
pub(crate) const FIXED_BACKGROUND: &str = "#0c0c0c";
pub(crate) const FIXED_FOREGROUND: &str = "#e8e8e8";
pub(crate) const FIXED_SURFACE: &str = "#1a1a1a";
pub(crate) const FIXED_OVERLAY: &str = "#d8d8d8";

/// Accent fallback when no pywal palette exists yet (fresh install, before
/// any wallpaper has been set for real): BOS's own bread-toned accents,
/// matching the curated default `colors.json` baked into every install.
const DEFAULT_COLOR1: &str = "#b98749";
const DEFAULT_COLOR2: &str = "#cd9450";
const DEFAULT_COLOR3: &str = "#e3a85c";
const DEFAULT_COLOR4: &str = "#eab672";
const DEFAULT_COLOR5: &str = "#f6c477";
const DEFAULT_COLOR6: &str = "#eabe82";

/// Full 8-colour pywal palette. background/foreground/color0/color7 are
/// BOS's fixed dark theme (see [`FIXED_BACKGROUND`] etc.); only color1-6
/// are ever pywal-derived.
#[derive(Debug, Clone)]
pub struct Palette {
    pub background: String,
    pub foreground: String,
    /// Fixed: darkest surface / overlay, never pywal-derived.
    pub color0: String,
    /// ANSI color1: red (pywal accent)
    pub color1: String,
    /// ANSI color2: green (pywal accent)
    pub color2: String,
    /// ANSI color3: yellow (pywal accent)
    pub color3: String,
    /// ANSI color4: blue / primary accent (pywal accent)
    pub color4: String,
    /// ANSI color5: pink / magenta (pywal accent)
    pub color5: String,
    /// ANSI color6: teal / cyan (pywal accent)
    pub color6: String,
    /// Fixed: light overlay / muted fg, never pywal-derived.
    pub color7: String,
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            background: FIXED_BACKGROUND.into(),
            foreground: FIXED_FOREGROUND.into(),
            color0: FIXED_SURFACE.into(),
            color1: DEFAULT_COLOR1.into(),
            color2: DEFAULT_COLOR2.into(),
            color3: DEFAULT_COLOR3.into(),
            color4: DEFAULT_COLOR4.into(),
            color5: DEFAULT_COLOR5.into(),
            color6: DEFAULT_COLOR6.into(),
            color7: FIXED_OVERLAY.into(),
        }
    }
}

#[derive(Deserialize)]
struct WalColors {
    #[serde(default)]
    colors: HashMap<String, String>,
}

/// Load palette from pywal's `colors.json`. Falls back to [`Palette::default`].
pub fn load_palette() -> Palette {
    let path = wal_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| from_wal_json(&s))
        .unwrap_or_default()
}

pub(crate) fn from_wal_json(json: &str) -> Option<Palette> {
    let wal: WalColors = serde_json::from_str(json).ok()?;
    let c = |k: &str, fallback: &str| -> String {
        wal.colors
            .get(k)
            .cloned()
            .unwrap_or_else(|| fallback.into())
    };
    Some(Palette {
        background: FIXED_BACKGROUND.into(),
        foreground: FIXED_FOREGROUND.into(),
        color0: FIXED_SURFACE.into(),
        color1: c("color1", DEFAULT_COLOR1),
        color2: c("color2", DEFAULT_COLOR2),
        color3: c("color3", DEFAULT_COLOR3),
        color4: c("color4", DEFAULT_COLOR4),
        color5: c("color5", DEFAULT_COLOR5),
        color6: c("color6", DEFAULT_COLOR6),
        color7: FIXED_OVERLAY.into(),
    })
}

fn wal_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("~/.cache"))
        .join("wal/colors.json")
}

/// pywal's cache file, `~/.cache/wal/colors.json`, the path [`load_palette`]
/// reads and [`write_wal_cache`] writes.
pub fn wal_cache_path() -> PathBuf {
    wal_path()
}

/// Serialise `palette` into pywal's `colors.json` cache shape.
///
/// pywal's cache is read by `wal`'s own templates and by anything else that
/// renders from `~/.cache/wal/colors.json` (terminal palettes, editor
/// themes), so all 16 ANSI slots plus `special` are written, not just the
/// six accents bread-theme itself consumes. `color8..color15` mirror
/// `color0..color7`: bread-theme has no bright variants, matching pywal's
/// own behaviour when brightening is off.
pub fn wal_cache_json(palette: &Palette) -> String {
    let slots = [
        &palette.color0,
        &palette.color1,
        &palette.color2,
        &palette.color3,
        &palette.color4,
        &palette.color5,
        &palette.color6,
        &palette.color7,
    ];
    let mut colors = serde_json::Map::with_capacity(16);
    for (i, hex) in slots.iter().enumerate() {
        let v = serde_json::Value::from(hex.as_str());
        colors.insert(format!("color{i}"), v.clone());
        colors.insert(format!("color{}", i + 8), v);
    }
    let doc = serde_json::json!({
        "wallpaper": "None",
        "alpha": "100",
        "special": {
            "background": palette.background,
            "foreground": palette.foreground,
            "cursor": palette.foreground,
        },
        "colors": colors,
    });
    // Serialising a `serde_json::Value` built from owned strings cannot fail.
    let mut s = serde_json::to_string_pretty(&doc).unwrap_or_default();
    s.push('\n');
    s
}

/// Write `palette` to `path` in pywal's `colors.json` shape, atomically
/// (temp file then rename), the same durability discipline the rest of
/// bread-theme's file writes use.
///
/// A consumer that already holds a per-output [`Palette`] (breadpaper's
/// `sync_focused_global`) can call this instead of shelling out to a second
/// `wal -i` just to refresh the cache: no subprocess spawn, and the cache
/// matches the palette that consumer computed rather than a fresh
/// re-extract from the image.
pub fn write_wal_cache(palette: &Palette, path: &Path) -> std::io::Result<()> {
    crate::output::atomic_write(path, &wal_cache_json(palette))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKYO_NIGHT: &str = r##"{
        "special": { "background": "#1a1b26", "foreground": "#c0caf5" },
        "colors": {
            "color0": "#15161e", "color1": "#f7768e", "color2": "#9ece6a",
            "color3": "#e0af68", "color4": "#7aa2f7", "color5": "#bb9af7",
            "color6": "#7dcfff", "color7": "#a9b1d6"
        }
    }"##;

    #[test]
    fn default_is_bos_fixed_dark_theme() {
        let p = Palette::default();
        assert_eq!(p.background, "#0c0c0c");
        assert_eq!(p.foreground, "#e8e8e8");
        assert_eq!(p.color0, "#1a1a1a");
        assert_eq!(p.color7, "#d8d8d8");
        assert_eq!(p.color4, "#eab672");
    }

    #[test]
    fn wal_json_background_and_surface_ignore_pywal() {
        // TOKYO_NIGHT's special/color0/color7 must NOT leak through: bg,
        // surface, overlay, and fg are always BOS's fixed dark values,
        // whatever pywal extracted from the wallpaper.
        let p = from_wal_json(TOKYO_NIGHT).unwrap();
        assert_eq!(p.background, "#0c0c0c");
        assert_eq!(p.foreground, "#e8e8e8");
        assert_eq!(p.color0, "#1a1a1a");
        assert_eq!(p.color7, "#d8d8d8");
    }

    #[test]
    fn wal_json_parses_accent_colors() {
        let p = from_wal_json(TOKYO_NIGHT).unwrap();
        assert_eq!(p.color1, "#f7768e");
        assert_eq!(p.color4, "#7aa2f7");
        assert_eq!(p.color6, "#7dcfff");
    }

    #[test]
    fn wal_json_missing_accent_color_uses_bos_default() {
        let p = from_wal_json(r#"{"colors":{}}"#).unwrap();
        assert_eq!(p.color4, "#eab672");
    }

    #[test]
    fn invalid_json_returns_none() {
        assert!(from_wal_json("not json").is_none());
        assert!(from_wal_json("").is_none());
    }

    #[test]
    fn empty_object_returns_bos_defaults() {
        let p = from_wal_json("{}").unwrap();
        assert_eq!(p.background, "#0c0c0c");
        assert_eq!(p.color4, "#eab672");
    }

    #[test]
    fn load_palette_returns_valid_hex_strings() {
        let p = load_palette();
        for val in [&p.background, &p.foreground, &p.color0, &p.color4] {
            assert!(val.starts_with('#'), "expected hex, got: {val}");
        }
    }

    fn distinct_accent_palette() -> Palette {
        Palette {
            color1: "#111111".into(),
            color2: "#222222".into(),
            color3: "#333333".into(),
            color4: "#444444".into(),
            color5: "#555555".into(),
            color6: "#666666".into(),
            ..Palette::default()
        }
    }

    #[test]
    fn wal_cache_json_roundtrips_through_from_wal_json() {
        let p = distinct_accent_palette();
        let back = from_wal_json(&wal_cache_json(&p)).expect("valid wal json");
        assert_eq!(back.color1, "#111111");
        assert_eq!(back.color4, "#444444");
        assert_eq!(back.color6, "#666666");
        // bg / fg / surface / overlay stay BOS fixed, as for any wal json.
        assert_eq!(back.background, FIXED_BACKGROUND);
        assert_eq!(back.foreground, FIXED_FOREGROUND);
        assert_eq!(back.color0, FIXED_SURFACE);
        assert_eq!(back.color7, FIXED_OVERLAY);
    }

    #[test]
    fn wal_cache_json_has_all_16_ansi_slots_and_special() {
        let p = distinct_accent_palette();
        let v: serde_json::Value = serde_json::from_str(&wal_cache_json(&p)).unwrap();

        let colors = v.get("colors").and_then(|c| c.as_object()).unwrap();
        for i in 0..16 {
            assert!(
                colors.contains_key(&format!("color{i}")),
                "missing color{i}"
            );
        }
        // color8..15 mirror color0..7.
        assert_eq!(colors["color9"], colors["color1"]);
        assert_eq!(colors["color15"], colors["color7"]);

        let special = v.get("special").and_then(|s| s.as_object()).unwrap();
        assert_eq!(special["background"], p.background);
        assert_eq!(special["foreground"], p.foreground);
        assert_eq!(special["cursor"], p.foreground);
        assert!(v.get("wallpaper").is_some());
        assert!(v.get("alpha").is_some());
    }

    #[test]
    fn write_wal_cache_writes_a_parseable_file_and_no_temp_left_behind() {
        let dir = std::env::temp_dir().join(format!(
            "bread-theme-walcache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("colors.json");

        write_wal_cache(&distinct_accent_palette(), &path).unwrap();

        let loaded = from_wal_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.color4, "#444444");

        let leftover: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(leftover.is_empty(), "leftover tmp files: {leftover:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
