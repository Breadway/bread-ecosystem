//! Per-output (per-monitor) palette and stylesheet paths under
//! `$XDG_RUNTIME_DIR/bread/{palettes,themes}/`.

use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::palette::{from_wal_json, Palette};
use crate::{load_palette, stylesheet, Typography};

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

/// Turn an output/connector name into a single path segment for
/// `palettes/<name>.json` / `themes/<name>.css`.
///
/// Keep `[A-Za-z0-9._-]` verbatim (every real Hyprland connector, `eDP-1`,
/// `HDMI-A-1`, `DP-2`, is already only those); **percent-encode** every other
/// byte as `%XX` (upper-hex). Percent-encoding is injective, so two different
/// connectors can never land on the same file: the old scheme mapped `/`, `:`
/// and space all to `_`, so `a/b` and `a:b` both became `a_b` and silently
/// shared one palette + stylesheet, last writer wins. `%` itself is not in the
/// keep-set, so it encodes to `%25` and the mapping stays reversible in
/// principle (nothing needs to decode today; [`reload_binds_for_sanitized`]
/// only compares `sanitize_output(name)` against the on-disk file stem).
pub fn sanitize_output(output: &str) -> String {
    if output.is_empty() {
        return "_".into();
    }
    let mut s = String::with_capacity(output.len());
    for b in output.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
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
    // file, e.g. two `bread-theme generate-output` runs writing the same
    // `themes/<output>.css` at once.
    let tmp = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => path.with_file_name(format!(".{name}.tmp.{}", std::process::id())),
        None => path.with_extension(format!("tmp.{}", std::process::id())),
    };
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Accents only: never persist pywal's light background/surface/overlay/fg.
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
    atomic_write(&path, &stylesheet(&Typography::active(), palette))?;
    Ok(path)
}

/// Like [`crate::write_shared_css`] but from an explicit palette.
pub fn write_shared_css_from(palette: &Palette) -> std::io::Result<PathBuf> {
    let path = crate::shared_css_path();
    atomic_write(&path, &stylesheet(&Typography::active(), palette))?;
    Ok(path)
}

/// Extract a [`Palette`] natively from an image, reproducing what `wal -i`
/// produced for it: `bread-palette-extract`'s port of pywal's ImageMagick
/// pipeline (`magick "<img>[0]" -resize 25% -colors N -unique-colors txt:-`,
/// `N` from 16 until at least 16 colours come back) with `color1..color6`
/// taken from entries 8..14 of the resulting list, as pywal does. This is
/// the one function in this crate whose binary contains ImageMagick- and
/// IJG-derived code (via the optional `bread-palette-extract` dependency),
/// which is why it, [`pywal_colors`] and [`generate_output`] are the only
/// things gated behind the `extract` feature; see `NOTICE` in
/// `bread-palette-extract` and this crate's licence section.
///
/// Deterministic: the same image always extracts to the same accents.
/// Images for which wal itself would give up (fewer than 16 distinct
/// colours) get bread's own curated default accents; that substitution is
/// bread's policy, not part of what wal/ImageMagick do, so it lives here
/// rather than in `bread-palette-extract`. Decode failures are an `Err`.
/// Replaces the old isolated `wal -i` subprocess: no external binary, no
/// temp `HOME`/`XDG_CACHE_HOME` dance.
#[cfg(feature = "extract")]
pub fn palette_from_image(path: &Path) -> std::io::Result<Palette> {
    let accents = match bread_palette_extract::wal_colors(path)? {
        Some(colors) => std::array::from_fn(|i| colors[8 + i].clone()),
        None => crate::palette::default_accents(),
    };
    Ok(crate::palette::from_accents(accents))
}

/// The raw ordered colour list pywal's `wal` backend builds for `path`
/// (lower-case `#rrggbb`, 16 or more entries; see
/// [`palette_from_image`]), or `None` where `wal` would exit with
/// "Imagemagick couldn't generate a suitable palette". Exposed for fidelity
/// comparisons against a real ImageMagick
/// (`bread-palette-extract/scripts/compare-pywal.py`).
#[cfg(feature = "extract")]
pub fn pywal_colors(path: &Path) -> std::io::Result<Option<Vec<String>>> {
    bread_palette_extract::wal_colors(path)
}

/// Persistent, cross-session cache dir: `$XDG_CACHE_HOME/bread` (falls back
/// to `~/.cache/bread` via [`dirs::cache_dir`], which itself honours `$HOME`
/// when `XDG_CACHE_HOME` is unset). Unlike [`runtime_bread_dir`]
/// (session-scoped, wiped at logout), this survives reboots: home for the
/// session-global palette cache and terminal sequences file, and a stable
/// directory for a long-lived watcher (breadman) to point a filesystem watch
/// at.
pub fn palette_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("~/.cache"))
        .join("bread")
}

/// Bread-owned global palette cache: replaces pywal's
/// `~/.cache/wal/colors.json` as the primary source [`crate::load_palette`]
/// reads. Written by [`write_global_palette`]; kept in the same wal
/// `colors.json` shape ([`crate::palette::wal_cache_json`]) so a copied wal
/// cache file, or anything written by [`crate::write_wal_cache`], is also
/// valid input here.
pub fn palette_cache_path() -> PathBuf {
    palette_cache_dir().join("palette.json")
}

/// Terminal colour sequences (OSC 4 for the 16 ANSI slots, OSC 10/11/12 for
/// fg/bg/cursor), written alongside the palette cache by
/// [`write_global_palette`]. BOS's skel `.zshrc` cats this file to colour new
/// terminal sessions; previously only `wal` ever wrote it, so terminals went
/// stale the moment `wal` left the picture.
pub fn sequences_path() -> PathBuf {
    palette_cache_dir().join("sequences")
}

/// OSC escape sequences for `palette`: OSC 4 for all 16 ANSI slots
/// (`color8..15` mirror `color0..7`, the same convention
/// [`crate::palette::wal_cache_json`] uses), then OSC 10/11/12 for
/// foreground/background/cursor. Terminated with ST (`ESC \`), the form
/// vte-based terminals, kitty, alacritty, and foot all accept.
fn sequences_string(palette: &Palette) -> String {
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
    let mut s = String::new();
    for (i, hex) in slots.iter().enumerate() {
        s.push_str(&format!("\x1b]4;{i};{hex}\x1b\\"));
        s.push_str(&format!("\x1b]4;{};{hex}\x1b\\", i + 8));
    }
    s.push_str(&format!("\x1b]10;{}\x1b\\", palette.foreground));
    s.push_str(&format!("\x1b]11;{}\x1b\\", palette.background));
    s.push_str(&format!("\x1b]12;{}\x1b\\", palette.foreground));
    s.push('\n');
    s
}

/// Write `palette` to [`palette_cache_path`] (wal `colors.json` shape) and
/// [`sequences_path`] (terminal OSC sequences), both atomically. This is
/// what breadpaper calls after computing a session-global palette, instead
/// of shelling out to `wal -i` just to refresh the cache.
pub fn write_global_palette(palette: &Palette) -> std::io::Result<()> {
    atomic_write(
        &palette_cache_path(),
        &crate::palette::wal_cache_json(palette),
    )?;
    atomic_write(&sequences_path(), &sequences_string(palette))?;
    Ok(())
}

/// [`palette_from_image`] + [`write_output_palette`] + [`write_output_css`].
#[cfg(feature = "extract")]
pub fn generate_output(output: &str, image: &Path) -> std::io::Result<PathBuf> {
    let palette = palette_from_image(image)?;
    write_output_palette(output, &palette)?;
    write_output_css(output, &palette)
}

#[cfg(test)]
pub(crate) static XDG_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Guards `XDG_CACHE_HOME` mutation for [`palette_cache_dir`]/[`load_palette`]
/// tests here and in `palette.rs`: a separate lock from [`XDG_ENV_LOCK`]
/// (which guards `XDG_RUNTIME_DIR`) since they're different env vars and
/// tests touching one shouldn't serialize behind the other.
#[cfg(test)]
pub(crate) static XDG_CACHE_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    fn sanitize_output_percent_encodes_unsafe_chars() {
        assert_eq!(sanitize_output("HDMI A:1"), "HDMI%20A%3A1");
        assert_eq!(sanitize_output("foo/bar"), "foo%2Fbar");
        assert_eq!(sanitize_output(""), "_");
        assert_eq!(sanitize_output("..ok_name-1"), "..ok_name-1");
        assert_eq!(sanitize_output("a%b"), "a%25b");
    }

    #[test]
    fn sanitize_output_is_injective_across_old_collisions() {
        // The old `_`-for-everything scheme collapsed all of these together.
        let names = ["a/b", "a:b", "a b", "a_b", "a%2Fb"];
        let mut seen = std::collections::HashSet::new();
        for n in names {
            assert!(
                seen.insert(sanitize_output(n)),
                "collision on {n} -> {}",
                sanitize_output(n)
            );
        }
    }

    #[test]
    fn output_paths_use_sanitize_and_sit_under_dirs() {
        let _lock = lock_xdg();
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1234");
        let css = output_css_path("HDMI A:1");
        let pal = output_palette_path("HDMI A:1");
        assert_eq!(css, themes_dir().join("HDMI%20A%3A1.css"));
        assert_eq!(pal, palettes_dir().join("HDMI%20A%3A1.json"));
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

    fn lock_xdg_cache_home() -> std::sync::MutexGuard<'static, ()> {
        XDG_CACHE_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` with `XDG_CACHE_HOME` pointed at a fresh temp dir (so
    /// [`palette_cache_dir`]/[`dirs::cache_dir`] and pywal's own
    /// `~/.cache/wal/colors.json` fallback both resolve under it), guarded
    /// by [`XDG_CACHE_HOME_LOCK`] so parallel tests can't race the env var.
    fn with_cache_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _lock = lock_xdg_cache_home();
        let dir = std::env::temp_dir().join(format!(
            "bread-theme-cache-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var("XDG_CACHE_HOME").ok();
        std::env::set_var("XDG_CACHE_HOME", &dir);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&dir)));
        match old {
            Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
            None => std::env::remove_var("XDG_CACHE_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        match result {
            Ok(v) => v,
            Err(e) => std::panic::resume_unwind(e),
        }
    }

    #[test]
    fn palette_cache_paths_sit_under_xdg_cache_home_bread() {
        with_cache_home(|dir| {
            assert_eq!(palette_cache_dir(), dir.join("bread"));
            assert_eq!(palette_cache_path(), dir.join("bread").join("palette.json"));
            assert_eq!(sequences_path(), dir.join("bread").join("sequences"));
        });
    }

    #[test]
    fn write_global_palette_writes_readable_palette_and_sequences() {
        with_cache_home(|_| {
            let p = Palette {
                color4: "#7aa2f7".into(),
                background: "#123456".into(),
                foreground: "#abcdef".into(),
                ..Default::default()
            };
            write_global_palette(&p).unwrap();

            let json = std::fs::read_to_string(palette_cache_path()).unwrap();
            let loaded = palette_from_json(&json).expect("palette.json must parse");
            // Fixed BOS values win over whatever's in the wal-shaped cache,
            // same as every other palette-from-JSON entry point.
            assert_eq!(loaded.background, FIXED_BACKGROUND);
            assert_eq!(loaded.color4, "#7aa2f7");

            let seq = std::fs::read_to_string(sequences_path()).unwrap();
            assert!(
                seq.contains("\x1b]4;4;#7aa2f7\x1b\\"),
                "missing OSC4 color4: {seq:?}"
            );
            assert!(
                seq.contains("\x1b]4;12;#7aa2f7\x1b\\"),
                "color12 must mirror color4: {seq:?}"
            );
            assert!(
                seq.contains("\x1b]10;#abcdef\x1b\\"),
                "missing OSC10 foreground: {seq:?}"
            );
            assert!(
                seq.contains("\x1b]11;#123456\x1b\\"),
                "missing OSC11 background: {seq:?}"
            );
            assert!(
                seq.contains("\x1b]12;#abcdef\x1b\\"),
                "missing OSC12 cursor: {seq:?}"
            );
        });
    }

    #[test]
    fn write_global_palette_leaves_no_temp_files_behind() {
        with_cache_home(|_| {
            write_global_palette(&Palette::default()).unwrap();
            for entry in std::fs::read_dir(palette_cache_dir()).unwrap() {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                assert!(!name.contains(".tmp."), "leftover tmp file: {name}");
            }
        });
    }

    #[test]
    fn load_palette_prefers_the_new_cache_over_the_wal_fallback() {
        with_cache_home(|dir| {
            // Only the wal fallback exists: load_palette must still find it.
            let wal_colors = dir.join("wal");
            std::fs::create_dir_all(&wal_colors).unwrap();
            std::fs::write(
                wal_colors.join("colors.json"),
                r##"{"colors":{"color4":"#111111"}}"##,
            )
            .unwrap();
            assert_eq!(crate::load_palette().color4, "#111111");

            // Once the new cache exists, it wins even though the wal file
            // is still there.
            write_global_palette(&Palette {
                color4: "#222222".into(),
                ..Default::default()
            })
            .unwrap();
            assert_eq!(crate::load_palette().color4, "#222222");
        });
    }
}

/// Tests for this crate's own gated API, not `bread-palette-extract`'s
/// internal `wal_colors` list. `bread-palette-extract/tests/golden.rs`
/// pins the full ordered colour list matches real pywal; it does not call
/// anything in `bread-theme`, so a bug in [`palette_from_image`]'s own
/// slicing (`[8..14]`), its `Palette` field mapping, or its
/// `default_accents()` fallback could ship with every one of those tests
/// green. These fixtures live in `bread-palette-extract` (the crate that
/// owns them); reached here via `CARGO_MANIFEST_DIR`'s sibling path rather
/// than duplicated, so there is exactly one copy of each golden file.
#[cfg(all(test, feature = "extract"))]
mod extract_feature_tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../bread-palette-extract/tests/fixtures")
            .join(name)
    }

    fn golden_accents(stem: &str) -> [String; 6] {
        let path = fixture("golden").join(format!("{stem}.json"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("missing golden fixture {path:?}: {e}"));
        let v: serde_json::Value =
            serde_json::from_str(&text).expect("golden fixture must be valid JSON");
        std::array::from_fn(|i| {
            v[format!("color{}", i + 1)]
                .as_str()
                .expect("golden fixture missing colorN")
                .to_lowercase()
        })
    }

    /// `palette_from_image` (the real public entry point breadpaper calls),
    /// not a re-implemented `[8..14]` slice, matches the golden accents
    /// exactly on two different formats (PNG and JPEG, so the JPEG port is
    /// exercised through this API too, not just through
    /// `bread-palette-extract`'s own tests).
    #[test]
    fn palette_from_image_matches_golden_accents() {
        for (image, golden_stem) in [
            ("bos-default.png", "bos-default"),
            ("bos-default.jpg", "bos-default.jpg"),
        ] {
            let palette =
                palette_from_image(&fixture(image)).unwrap_or_else(|e| panic!("{image}: {e}"));
            let want = golden_accents(golden_stem);
            let got = [
                palette.color1.to_lowercase(),
                palette.color2.to_lowercase(),
                palette.color3.to_lowercase(),
                palette.color4.to_lowercase(),
                palette.color5.to_lowercase(),
                palette.color6.to_lowercase(),
            ];
            assert_eq!(got, want, "{image}: accents via palette_from_image");
        }
    }

    /// An image wal itself would give up on (fewer than 16 distinct
    /// colours after resize/quantize) must come back as bread's curated
    /// defaults, not an error and not six copies of one colour.
    #[test]
    fn palette_from_image_falls_back_to_defaults_under_16_colours() {
        let flat = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flat.png");
        let palette = palette_from_image(&flat)
            .expect("a flat image is a successful extraction, not a decode error");
        let default = crate::palette::default_accents();
        assert_eq!(palette.color1, default[0]);
        assert_eq!(palette.color2, default[1]);
        assert_eq!(palette.color3, default[2]);
        assert_eq!(palette.color4, default[3]);
        assert_eq!(palette.color5, default[4]);
        assert_eq!(palette.color6, default[5]);
    }

    /// A genuine decode failure must stay an `Err`, not be swallowed into
    /// the default-accent fallback: only "wal would have given up" (too
    /// few colours) gets defaults, per [`palette_from_image`]'s own doc.
    #[test]
    fn palette_from_image_propagates_decode_errors() {
        let dir = std::env::temp_dir().join(format!(
            "bread-theme-extract-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not-an-image.png");
        std::fs::write(&path, b"this is not image data at all").unwrap();

        assert!(
            palette_from_image(&path).is_err(),
            "garbage input must be an error, not a default-accent fallback"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
