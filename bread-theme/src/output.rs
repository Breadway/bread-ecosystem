//! Per-output (per-monitor) palette and stylesheet paths under
//! `$XDG_RUNTIME_DIR/bread/{palettes,themes}/`.

use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::palette::{from_wal_json, Palette};
use crate::{load_palette, stylesheet};

/// Session-scoped `$XDG_RUNTIME_DIR/bread`, same fallback as [`crate::shared_css_path`].
pub(crate) fn runtime_bread_dir() -> PathBuf {
    // XDG spec: `XDG_RUNTIME_DIR` is only honored when set to a non-empty,
    // *absolute* path; a relative value must be ignored.
    if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(&rt);
        if !rt.is_empty() && p.is_absolute() {
            return p.join("bread");
        }
    }
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("bread")
}

/// Keep `[A-Za-z0-9._-]`; replace everything else with `_`.
pub fn sanitize_output(output: &str) -> String {
    let s: String = output
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "_".into()
    } else {
        s
    }
}

pub fn themes_dir() -> PathBuf {
    runtime_bread_dir().join("themes")
}

pub fn palettes_dir() -> PathBuf {
    runtime_bread_dir().join("palettes")
}

pub fn output_css_path(output: &str) -> PathBuf {
    themes_dir().join(format!("{}.css", sanitize_output(output)))
}

pub fn output_palette_path(output: &str) -> PathBuf {
    palettes_dir().join(format!("{}.json", sanitize_output(output)))
}

pub(crate) fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Pad the temp name with the pid (matching `bread_utils::atomic`) so two
    // concurrent writers for the same target can't race on one shared `.tmp`
    // file — e.g. two `bread-theme generate-output` runs writing the same
    // `themes/<output>.css` at once.
    let tmp = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => path.with_file_name(format!(".{name}.tmp.{}", std::process::id())),
        None => path.with_extension(format!("tmp.{}", std::process::id())),
    };
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Accents only — never persist pywal's light background/surface/overlay/fg.
#[derive(Serialize)]
struct StoredColors {
    color1: String,
    color2: String,
    color3: String,
    color4: String,
    color5: String,
    color6: String,
}

#[derive(Serialize)]
struct StoredPalette {
    colors: StoredColors,
}

/// Parse on-disk JSON: wal `colors.json` shape, or a flat `{color1..color6}` object.
/// Always forces FIXED background/foreground/color0/color7 via [`from_wal_json`].
pub fn palette_from_json(json: &str) -> Option<Palette> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    if value
        .get("colors")
        .and_then(|c| c.as_object())
        .is_some_and(|o| !o.is_empty())
    {
        return from_wal_json(json);
    }
    if value.get("color1").is_some()
        || value.get("color2").is_some()
        || value.get("color3").is_some()
        || value.get("color4").is_some()
        || value.get("color5").is_some()
        || value.get("color6").is_some()
    {
        let wrapped = serde_json::json!({ "colors": value });
        return from_wal_json(&wrapped.to_string());
    }
    from_wal_json(json)
}

/// Load `palettes/<output>.json`; fall back to [`load_palette`].
pub fn load_palette_for(output: &str) -> Palette {
    std::fs::read_to_string(output_palette_path(output))
        .ok()
        .and_then(|s| palette_from_json(&s))
        .unwrap_or_else(load_palette)
}

pub fn write_output_palette(output: &str, palette: &Palette) -> std::io::Result<PathBuf> {
    let path = output_palette_path(output);
    let stored = StoredPalette {
        colors: StoredColors {
            color1: palette.color1.clone(),
            color2: palette.color2.clone(),
            color3: palette.color3.clone(),
            color4: palette.color4.clone(),
            color5: palette.color5.clone(),
            color6: palette.color6.clone(),
        },
    };
    let json = serde_json::to_string_pretty(&stored)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    atomic_write(&path, &json)?;
    Ok(path)
}

pub fn write_output_css(output: &str, palette: &Palette) -> std::io::Result<PathBuf> {
    let path = output_css_path(output);
    atomic_write(&path, &stylesheet(palette))?;
    Ok(path)
}

/// Like [`crate::write_shared_css`] but from an explicit palette.
pub fn write_shared_css_from(palette: &Palette) -> std::io::Result<PathBuf> {
    let path = crate::shared_css_path();
    atomic_write(&path, &stylesheet(palette))?;
    Ok(path)
}

/// Isolated `wal -i <image> -n -q` with `XDG_CACHE_HOME` set to a unique temp
/// dir so the user's `~/.cache/wal` is not clobbered.
pub fn palette_from_image(path: &Path) -> std::io::Result<Palette> {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!("bread-theme-wal-{pid}-{nanos}"));
    std::fs::create_dir_all(&tmp)?;
    struct Rm(PathBuf);
    impl Drop for Rm {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _guard = Rm(tmp.clone());

    // Classic pywal ignores XDG_CACHE_HOME and writes $HOME/.cache/wal.
    // Point HOME at the temp dir so a per-output extract cannot clobber
    // the session cache (or the other monitor's last `wal -i`).
    let status = match std::process::Command::new("wal")
        .arg("-i")
        .arg(path)
        .args(["-n", "-q"])
        .env("HOME", &tmp)
        .env("XDG_CACHE_HOME", tmp.join(".cache"))
        .status()
    {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "wal is not installed",
            ));
        }
        Err(e) => return Err(e),
        Ok(s) => s,
    };
    if !status.success() {
        return Err(std::io::Error::other(format!("wal failed with {status}")));
    }

    let json_path = [
        tmp.join(".cache").join("wal").join("colors.json"),
        tmp.join("wal").join("colors.json"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "wal did not write colors.json under the isolated cache",
        )
    })?;
    let json = std::fs::read_to_string(&json_path)?;
    from_wal_json(&json).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "wal produced unparseable colors.json",
        )
    })
}

/// [`palette_from_image`] + [`write_output_palette`] + [`write_output_css`].
pub fn generate_output(output: &str, image: &Path) -> std::io::Result<PathBuf> {
    let palette = palette_from_image(image)?;
    write_output_palette(output, &palette)?;
    write_output_css(output, &palette)
}

#[cfg(test)]
pub(crate) static XDG_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{FIXED_BACKGROUND, FIXED_FOREGROUND, FIXED_OVERLAY, FIXED_SURFACE};

    fn lock_xdg() -> std::sync::MutexGuard<'static, ()> {
        XDG_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn with_runtime_dir<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _lock = lock_xdg();
        let dir = std::env::temp_dir().join(format!(
            "bread-theme-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var("XDG_RUNTIME_DIR").ok();
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&dir)));
        match old {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        match result {
            Ok(v) => v,
            Err(e) => std::panic::resume_unwind(e),
        }
    }

    #[test]
    fn sanitize_output_keeps_hyprland_connectors() {
        assert_eq!(sanitize_output("HDMI-A-1"), "HDMI-A-1");
        assert_eq!(sanitize_output("eDP-1"), "eDP-1");
        assert_eq!(sanitize_output("DP-2"), "DP-2");
    }

    #[test]
    fn sanitize_output_replaces_unsafe_chars() {
        assert_eq!(sanitize_output("HDMI A:1"), "HDMI_A_1");
        assert_eq!(sanitize_output("foo/bar"), "foo_bar");
        assert_eq!(sanitize_output(""), "_");
        assert_eq!(sanitize_output("..ok_name-1"), "..ok_name-1");
    }

    #[test]
    fn output_paths_use_sanitize_and_sit_under_dirs() {
        let _lock = lock_xdg();
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1234");
        let css = output_css_path("HDMI A:1");
        let pal = output_palette_path("HDMI A:1");
        assert_eq!(css, themes_dir().join("HDMI_A_1.css"));
        assert_eq!(pal, palettes_dir().join("HDMI_A_1.json"));
        assert!(css.starts_with(themes_dir()));
        assert!(pal.starts_with(palettes_dir()));
        assert_eq!(
            output_css_path("eDP-1"),
            PathBuf::from("/run/user/1234/bread/themes/eDP-1.css")
        );
    }

    #[test]
    fn load_palette_for_missing_file_has_fixed_bg() {
        with_runtime_dir(|_| {
            let p = load_palette_for("no-such-output");
            assert_eq!(p.background, FIXED_BACKGROUND);
            assert!(p.color4.starts_with('#'));
        });
    }

    #[test]
    fn write_output_palette_roundtrips_color4() {
        with_runtime_dir(|_| {
            let p = Palette {
                color4: "#7aa2f7".into(),
                background: "#ffffff".into(),
                ..Default::default()
            };
            write_output_palette("HDMI-A-1", &p).unwrap();
            let loaded = load_palette_for("HDMI-A-1");
            assert_eq!(loaded.color4, "#7aa2f7");
            assert_eq!(loaded.background, FIXED_BACKGROUND);
            assert_eq!(loaded.foreground, FIXED_FOREGROUND);
            assert_eq!(loaded.color0, FIXED_SURFACE);
            assert_eq!(loaded.color7, FIXED_OVERLAY);
        });
    }

    #[test]
    fn write_shared_css_from_writes_shared_css_path() {
        with_runtime_dir(|rt| {
            let path = write_shared_css_from(&Palette::default()).unwrap();
            assert_eq!(path, crate::shared_css_path());
            assert_eq!(path, rt.join("bread").join("theme.css"));
            let css = std::fs::read_to_string(&path).unwrap();
            assert!(css.contains("@define-color accent "));
        });
    }

    #[test]
    fn load_palette_for_accepts_flat_color_object() {
        with_runtime_dir(|_| {
            let path = output_palette_path("DP-1");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, r##"{"color4":"#112233","color1":"#abcdef"}"##).unwrap();
            let p = load_palette_for("DP-1");
            assert_eq!(p.color4, "#112233");
            assert_eq!(p.color1, "#abcdef");
            assert_eq!(p.background, FIXED_BACKGROUND);
        });
    }
}
