use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Full 8-colour pywal palette. Catppuccin Mocha is the fallback.
#[derive(Debug, Clone)]
pub struct Palette {
    pub background: String,
    pub foreground: String,
    /// ANSI color0 — darkest surface / overlay
    pub color0: String,
    /// ANSI color1 — red
    pub color1: String,
    /// ANSI color2 — green
    pub color2: String,
    /// ANSI color3 — yellow
    pub color3: String,
    /// ANSI color4 — blue (primary accent)
    pub color4: String,
    /// ANSI color5 — pink / magenta
    pub color5: String,
    /// ANSI color6 — teal / cyan
    pub color6: String,
    /// ANSI color7 — light overlay / muted fg
    pub color7: String,
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            background: "#1e1e2e".into(),
            foreground: "#cdd6f4".into(),
            color0: "#45475a".into(),
            color1: "#f38ba8".into(),
            color2: "#a6e3a1".into(),
            color3: "#f9e2af".into(),
            color4: "#89b4fa".into(),
            color5: "#f5c2e7".into(),
            color6: "#94e2d5".into(),
            color7: "#bac2de".into(),
        }
    }
}

#[derive(Deserialize)]
struct WalColors {
    #[serde(default)]
    colors: HashMap<String, String>,
    special: Option<WalSpecial>,
}

#[derive(Deserialize)]
struct WalSpecial {
    background: Option<String>,
    foreground: Option<String>,
}

/// Load palette from pywal's `colors.json`. Falls back to Catppuccin Mocha.
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
        wal.colors.get(k).cloned().unwrap_or_else(|| fallback.into())
    };
    Some(Palette {
        background: wal.special.as_ref().and_then(|s| s.background.clone())
            .unwrap_or_else(|| "#1e1e2e".into()),
        foreground: wal.special.as_ref().and_then(|s| s.foreground.clone())
            .unwrap_or_else(|| "#cdd6f4".into()),
        color0: c("color0", "#45475a"),
        color1: c("color1", "#f38ba8"),
        color2: c("color2", "#a6e3a1"),
        color3: c("color3", "#f9e2af"),
        color4: c("color4", "#89b4fa"),
        color5: c("color5", "#f5c2e7"),
        color6: c("color6", "#94e2d5"),
        color7: c("color7", "#bac2de"),
    })
}

fn wal_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("~/.cache"))
        .join("wal/colors.json")
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
    fn default_is_catppuccin_mocha() {
        let p = Palette::default();
        assert_eq!(p.background, "#1e1e2e");
        assert_eq!(p.foreground, "#cdd6f4");
        assert_eq!(p.color4, "#89b4fa");
    }

    #[test]
    fn wal_json_parses_special() {
        let p = from_wal_json(TOKYO_NIGHT).unwrap();
        assert_eq!(p.background, "#1a1b26");
        assert_eq!(p.foreground, "#c0caf5");
    }

    #[test]
    fn wal_json_parses_colors() {
        let p = from_wal_json(TOKYO_NIGHT).unwrap();
        assert_eq!(p.color0, "#15161e");
        assert_eq!(p.color4, "#7aa2f7");
        assert_eq!(p.color7, "#a9b1d6");
    }

    #[test]
    fn wal_json_missing_special_uses_catppuccin_fallback() {
        let p = from_wal_json(r#"{"colors":{}}"#).unwrap();
        assert_eq!(p.background, "#1e1e2e");
        assert_eq!(p.foreground, "#cdd6f4");
    }

    #[test]
    fn wal_json_missing_color_uses_catppuccin_fallback() {
        let p = from_wal_json(r##"{"special":{"background":"#ff0000","foreground":"#ffffff"},"colors":{}}"##).unwrap();
        assert_eq!(p.color4, "#89b4fa");
    }

    #[test]
    fn invalid_json_returns_none() {
        assert!(from_wal_json("not json").is_none());
        assert!(from_wal_json("").is_none());
    }

    #[test]
    fn empty_object_returns_all_defaults() {
        let p = from_wal_json("{}").unwrap();
        assert_eq!(p.background, "#1e1e2e");
    }

    #[test]
    fn load_palette_returns_valid_hex_strings() {
        let p = load_palette();
        for val in [&p.background, &p.foreground, &p.color0, &p.color4] {
            assert!(val.starts_with('#'), "expected hex, got: {val}");
        }
    }
}
