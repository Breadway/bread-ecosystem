//! Golden-fixture fidelity tests for [`bread_palette_extract::wal_colors`]
//! (ECO-29/ECO-30): reference images under `tests/fixtures/`, each with
//! golden output captured from real pywal by `scripts/capture-goldens.py`
//! (`color1..color6` from `wal -i` run in an isolated `HOME`, plus
//! `wal_colors`, the full ordered list pywal's backend built). This crate is
//! a port of the ImageMagick pipeline pywal runs, so these are exact
//! assertions on every slot. The wider real-wallpaper comparison is
//! `scripts/compare-pywal.py`.

use std::path::PathBuf;

/// Every committed fixture with a golden: PNG (8-bit RGB, RGBA, 4-bit
/// palette, 8-bit gray, 16-bit gray), baseline and progressive JPEG, lossy
/// WebP, 24-bit and palette BMP, and an animated GIF.
const FIXTURES: &[&str] = &[
    "bos-default.png",
    "bright-screenshot.png",
    "dark-moody.png",
    "near-monochrome.png",
    "near-monochrome-16bit.png",
    "saturated-colorful.png",
    "bos-default.jpg",
    "bos-default-progressive.jpg",
    "bos-default.webp",
    "bos-default.bmp",
    "bos-default-palette.bmp",
    "bos-default-animated.gif",
];

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn golden(name: &str) -> serde_json::Value {
    let stem = name.strip_suffix(".png").unwrap_or(name);
    let path = fixture_path("golden").join(format!("{stem}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing golden fixture {path:?}: {e}"));
    serde_json::from_str(&text).expect("golden fixture must be valid JSON")
}

fn golden_accents(name: &str) -> Vec<String> {
    let v = golden(name);
    (1..=6)
        .map(|i| {
            v[format!("color{i}")]
                .as_str()
                .expect("golden fixture missing colorN")
                .to_lowercase()
        })
        .collect()
}

/// `color1..color6`: entries `8..14` of [`bread_palette_extract::wal_colors`]'
/// list, exactly as bread-theme's `extract` feature slices them. This crate
/// deliberately has no public helper for that slice (see the module docs:
/// slicing and the default-accent fallback are bread-theme's policy, not
/// part of what wal/ImageMagick do), so it is re-implemented here to pin
/// this crate's own contract, `wal_colors`'s full ordered list. The real
/// consumer API, `bread_theme::palette_from_image`, is covered separately
/// by `bread-theme/src/output.rs`'s `extract_feature_tests` module.
fn accents(name: &str) -> Vec<String> {
    let list = bread_palette_extract::wal_colors(&fixture_path(name))
        .unwrap()
        .expect("wal produced a palette for this fixture");
    list[8..14].iter().map(|h| h.to_lowercase()).collect()
}

#[test]
fn accents_match_real_wal_exactly() {
    for name in FIXTURES {
        assert_eq!(accents(name), golden_accents(name), "{name}");
    }
}

#[test]
fn full_colour_list_matches_real_wal_exactly() {
    for name in FIXTURES {
        let want: Vec<String> = golden(name)["wal_colors"]
            .as_array()
            .expect("golden fixture missing wal_colors")
            .iter()
            .map(|c| c.as_str().unwrap().to_lowercase())
            .collect();
        let got = bread_palette_extract::wal_colors(&fixture_path(name))
            .unwrap()
            .expect("wal produced a palette for this fixture");
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn sixteen_bit_input_reproduces_pywals_hex_truncation() {
    // near-monochrome-16bit.png is a grey gradient stored as 16-bit
    // grayscale. ImageMagick's txt: output prints it as #RRRRGGGGBBBB
    // and pywal's `#.{6}` regex keeps "#RRRRGG": red's high byte, red's
    // low byte, green's high byte. That is why real wal gives green and
    // purple accents for a grey image. This extractor reproduces the quirk
    // on purpose (matching what users got from pywal is the point); the
    // same gradient saved as 8-bit (near-monochrome.png) yields greys.
    let wide = accents("near-monochrome-16bit.png");
    assert_eq!(wide[2], "#78f078");
    let narrow = accents("near-monochrome.png");
    for hex in narrow {
        assert!(
            hex[1..3] == hex[3..5] && hex[3..5] == hex[5..7],
            "{hex} is not grey"
        );
    }
}

#[test]
fn gif_uses_the_first_frame_only() {
    // wal reads "<img>[0]". The fixture's second frame is a different
    // picture, so extracting from it must give different accents.
    use image::AnimationDecoder;
    let file = std::fs::File::open(fixture_path("bos-default-animated.gif")).unwrap();
    let decoder = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(file)).unwrap();
    let frames = decoder.into_frames().collect_frames().unwrap();
    assert!(frames.len() >= 2);
    let second = std::env::temp_dir().join(format!(
        "bread-palette-extract-gif-frame1-{}.png",
        std::process::id()
    ));
    frames[1].buffer().save(&second).unwrap();
    let from_gif = accents("bos-default-animated.gif");
    let from_second = accents_from_path(&second);
    let _ = std::fs::remove_file(&second);
    assert_eq!(from_gif, golden_accents("bos-default-animated.gif"));
    assert_ne!(from_gif, from_second);
}

fn accents_from_path(path: &std::path::Path) -> Vec<String> {
    let list = bread_palette_extract::wal_colors(path)
        .unwrap()
        .expect("wal produced a palette for this image");
    list[8..14].iter().map(|h| h.to_lowercase()).collect()
}
