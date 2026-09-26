//! Native (no `wal`/ImageMagick subprocess) accent extraction from an image,
//! reimplementing wal's real pipeline as closely as practical so switching
//! from `wal -i` to this extractor changes accents as little as possible.
//! See [`crate::output::palette_from_image`], the only public entry point;
//! everything here is crate-private.
//!
//! What `wal -i <img> -n -q` (the default "wal" backend, dark mode, the only
//! one bread-theme ever shelled out to) actually does, read from the
//! installed `pywal` source (`pywal/backends/wal.py`, `pywal/colors.py`):
//!
//!  1. `magick "<img>[0]" -resize 25% -colors N -unique-colors txt:-`, with
//!     `N` starting at 16 and retried up to 35, stopping at the first `N`
//!     whose output has more than 16 lines (a header line plus at least 16
//!     distinct colours). If nothing in that range clears 16 distinct
//!     colours, real wal gives up and exits with an error.
//!  2. The unique colours, in ImageMagick's output order, become `colors`.
//!     `pywal/colors.py::colors_to_dict` maps `color1..color6` straight from
//!     `colors[1]..colors[6]` of what the backend returns; `wal.py::adjust`
//!     builds that list as `colors[:1] + colors[8:16] + colors[8:-1]` and
//!     (in dark mode) only ever touches indices 0, 7, 8, 15 afterward. Working
//!     through the slice arithmetic: `color1..color6 == colors[8]..colors[13]`
//!     of the original ImageMagick list, untouched by any of wal's own
//!     adjustments. bg/fg/color0/color7 are BOS's fixed values regardless
//!     (see `palette.rs`), so only that one slice of the list matters here.
//!
//! This module reproduces steps 1-2 with a resize + a from-scratch octree
//! colour quantizer, **not** a byte-for-byte port of ImageMagick's
//! `quantize.c`: that algorithm also dithers by default, and its exact
//! node-merge priority and `-unique-colors` traversal order are internal,
//! version-specific C implementation details not derivable from the CLI
//! (and not worth pinning bread-theme to one ImageMagick build's behaviour
//! even if they were). The quantizer here is a standard bottom-up octree
//! colour quantizer: distinct colours are grouped by their shared
//! high-order RGB bits ("which octree node would contain this colour at
//! depth `d`"), coarsened one bit level at a time until the group count
//! fits the target, then walked in a fixed Z-order to produce the ordered
//! `colors` list. See the bread-theme CHANGELOG and ECO-30 for measured
//! fidelity against real `wal` output on both the committed fixtures and a
//! wider set of real wallpapers.
//!
//! Determinism: no RNG. The `HashMap` used to build the colour histogram has
//! randomized iteration order, but every bucket derived from it is
//! re-sorted by its fixed Z-order key ([`bucket_key`]) before use, so the
//! final `colors` list order never depends on hash iteration order.

use image::{DynamicImage, GenericImageView};
use std::collections::HashMap;
use std::io;
use std::path::Path;

/// Alpha below this is "not really there": a transparent border shouldn't
/// drag the palette toward black.
const ALPHA_OPAQUE_THRESHOLD: u8 = 16;

/// wal's own retry range (`pywal/backends/wal.py::gen_colors`): start at 16,
/// try up to 35, succeed the first time the quantized result has at least
/// 16 distinct colours.
const MIN_TARGET: u32 = 16;
const MAX_TARGET: u32 = 35;

pub(crate) fn accents_from_image(path: &Path) -> io::Result<[String; 6]> {
    let img = image::open(path).map_err(|e| match e {
        image::ImageError::IoError(io_err) => io_err,
        other => io::Error::new(io::ErrorKind::InvalidData, other.to_string()),
    })?;
    let resized = resize_quarter(&img);
    let histogram = opaque_histogram(&resized);
    if histogram.is_empty() {
        return Ok(crate::palette::default_accents());
    }
    let colors = quantize_like_wal(&histogram);
    Ok(accents_from_colors(&colors))
}

/// wal always resizes to 25% before quantizing (`-resize 25%` in
/// `wal.py::imagemagick`), regardless of the source resolution: a 4K
/// wallpaper and a small crop of the same photo extract to comparable
/// accents, and quantizing over roughly 1/16th the pixels keeps this fast on
/// large wallpapers.
///
/// `image`'s `Lanczos3` filter was picked empirically, not read out of
/// ImageMagick's source: resizing the same photographic fixture with each
/// filter `image` offers and diffing pixel-for-pixel against a real
/// `magick -resize 25%` run showed Lanczos3 matching a real downscaled
/// photo almost exactly (MSE ~0 on `tests/fixtures/bos-default.png`), while
/// Nearest/Triangle/CatmullRom/Gaussian were all visibly further off. It's a
/// worse match on the hard-edged synthetic fixtures (flat colour blocks
/// have no "correct" resampling filter to match against), but real
/// wallpapers are photos, not flat colour blocks, so this module optimizes
/// for the photo case.
fn resize_quarter(img: &DynamicImage) -> DynamicImage {
    let (w, h) = img.dimensions();
    let nw = ((w as f64) * 0.25).round().max(1.0) as u32;
    let nh = ((h as f64) * 0.25).round().max(1.0) as u32;
    img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3)
}

/// Histogram of opaque pixel colours in the resized image: `color -> pixel
/// count`. Iteration order is unspecified (`HashMap`); every consumer below
/// re-derives a fixed order from [`bucket_key`], so that doesn't leak into
/// the result.
fn opaque_histogram(img: &DynamicImage) -> HashMap<[u8; 3], u64> {
    let rgba = img.to_rgba8();
    let mut hist = HashMap::new();
    for p in rgba.pixels() {
        if p.0[3] >= ALPHA_OPAQUE_THRESHOLD {
            *hist.entry([p.0[0], p.0[1], p.0[2]]).or_insert(0u64) += 1;
        }
    }
    hist
}

/// One octree bucket at some depth: the weighted-average colour of every
/// distinct source colour that shares this node, plus the node's own
/// Z-order key ([`bucket_key`]) for a fixed traversal order.
struct Bucket {
    key: u32,
    weight: u64,
    r: u64,
    g: u64,
    b: u64,
}

impl Bucket {
    fn color(&self) -> [u8; 3] {
        let w = self.weight.max(1);
        [(self.r / w) as u8, (self.g / w) as u8, (self.b / w) as u8]
    }
}

/// Interleave the top `depth` bits of each channel (MSB first, 3 bits per
/// level: R, G, B) into a single key: the octree node address containing
/// this colour at that depth. Two colours share a node at depth `d` exactly
/// when `bucket_key(a, d) == bucket_key(b, d)`, so grouping the histogram by
/// this key is grouping colours into octree nodes; sorting by it walks the
/// tree in a fixed, deterministic order (a stand-in for ImageMagick's own
/// colormap traversal order, whose exact form is a C implementation detail
/// this module doesn't attempt to reproduce bit-for-bit).
fn bucket_key(color: [u8; 3], depth: u32) -> u32 {
    let mut key = 0u32;
    for level in 0..depth {
        let bit = 7 - level;
        let rb = ((color[0] >> bit) & 1) as u32;
        let gb = ((color[1] >> bit) & 1) as u32;
        let bb = ((color[2] >> bit) & 1) as u32;
        key = (key << 3) | (rb << 2 | gb << 1 | bb);
    }
    key
}

/// Bucket the histogram at octree depth `depth` (0 = the whole image is one
/// bucket, 8 = every distinct colour is its own bucket), ordered by
/// [`bucket_key`].
fn buckets_at_depth(histogram: &HashMap<[u8; 3], u64>, depth: u32) -> Vec<Bucket> {
    let mut by_key: HashMap<u32, Bucket> = HashMap::new();
    for (&color, &weight) in histogram {
        let key = bucket_key(color, depth);
        let b = by_key.entry(key).or_insert(Bucket {
            key,
            weight: 0,
            r: 0,
            g: 0,
            b: 0,
        });
        b.weight += weight;
        b.r += color[0] as u64 * weight;
        b.g += color[1] as u64 * weight;
        b.b += color[2] as u64 * weight;
    }
    let mut buckets: Vec<Bucket> = by_key.into_values().collect();
    buckets.sort_by_key(|b| b.key);
    buckets
}

/// Reproduce wal's quantize-and-retry loop: for `target` in `16..=35`, find
/// the finest octree depth whose bucket count is `<= target` (bottom-up
/// level collapse, coarsening from full 24-bit precision), and accept the
/// first `target` whose result has `>= 16` buckets (wal's own
/// `len(raw_colors) > 16` check once the header line is accounted for).
///
/// If nothing in that range clears 16 buckets, the image genuinely has
/// fewer than 16 distinguishable colours after the 25% resize: exactly the
/// case where real wal gives up and exits with an error. Unlike wal,
/// [`accents_from_colors`] must always return something usable, so it
/// treats a too-short result as "no real palette" and falls back to bread's
/// curated defaults rather than erroring or shipping a degenerate palette.
fn quantize_like_wal(histogram: &HashMap<[u8; 3], u64>) -> Vec<[u8; 3]> {
    // Precompute buckets at every depth once (9 passes over the histogram,
    // depth 8 down to 0) instead of redoing a full depth-8-downward search
    // per retried target: `buckets_at_depth(d).len()` only depends on `d`,
    // not on which target we're currently trying, so there's no need to
    // recompute it 20 times (once per target in wal's 16..=35 retry range).
    let by_depth: Vec<Vec<Bucket>> = (0..=8).map(|d| buckets_at_depth(histogram, d)).collect();
    for target in MIN_TARGET..=MAX_TARGET {
        // Finest (largest) depth whose bucket count still fits this target.
        if let Some(buckets) = by_depth
            .iter()
            .rev()
            .find(|buckets| buckets.len() as u32 <= target)
        {
            if buckets.len() >= 16 {
                return buckets.iter().map(Bucket::color).collect();
            }
        }
    }
    // Nothing in wal's retry range reached 16 distinct colours: the image
    // genuinely doesn't have that much colour diversity even at full
    // precision (depth 8, index 8 of `by_depth`). Return whatever that
    // finest level has; `accents_from_colors` falls back to bread's
    // defaults for anything still short of 16.
    by_depth[8].iter().map(Bucket::color).collect()
}

/// `color1..color6` are `colors[8]..colors[13]` of the ordered list (see the
/// module docs for the derivation from pywal's own source). `colors` is
/// only ever shorter than 16 when [`quantize_like_wal`] couldn't reach wal's
/// own success threshold, i.e. wal would have errored on this image; fall
/// back to bread's curated defaults in that case instead.
fn accents_from_colors(colors: &[[u8; 3]]) -> [String; 6] {
    if colors.len() < 16 {
        return crate::palette::default_accents();
    }
    std::array::from_fn(|i| to_hex(colors[8 + i]))
}

fn to_hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// `(hue in [0, 360), saturation in [0, 1], lightness in [0, 1])`. Used by
/// the golden-fixture fidelity tests in `output.rs` to compare hue/
/// saturation family, not by the extraction pipeline itself: test-only.
#[cfg(test)]
pub(crate) fn rgb_to_hsl([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let raw_h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    let h = (raw_h * 60.0).rem_euclid(360.0);
    (h, s, l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(path: &Path, w: u32, h: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) {
        let mut img = image::RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.put_pixel(x, y, image::Rgba(pixel(x, y)));
            }
        }
        img.save(path).unwrap();
    }

    fn tmp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "bread-theme-extract-test-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn rgb_to_hsl_primary_colors() {
        assert_eq!(rgb_to_hsl([255, 0, 0]).0, 0.0);
        let (h, s, l) = rgb_to_hsl([0, 255, 0]);
        assert!((h - 120.0).abs() < 0.1, "green hue: {h}");
        assert!(s > 0.99 && (l - 0.5).abs() < 0.01);
        let (h, _, _) = rgb_to_hsl([0, 0, 255]);
        assert!((h - 240.0).abs() < 0.1, "blue hue: {h}");
    }

    #[test]
    fn rgb_to_hsl_gray_has_zero_saturation() {
        let (_, s, l) = rgb_to_hsl([128, 128, 128]);
        assert_eq!(s, 0.0);
        assert!((l - 0.502).abs() < 0.01);
    }

    #[test]
    fn bucket_key_groups_colors_sharing_a_prefix() {
        // Same top 4 bits per channel (0xA0.., 0xA0.., 0xA0..): must share a
        // depth-4 node.
        assert_eq!(
            bucket_key([0xA3, 0xA7, 0xA1], 4),
            bucket_key([0xAF, 0xA0, 0xAE], 4)
        );
        // Differ in the top bit of red: must NOT share a depth-1 node.
        assert_ne!(bucket_key([0x00, 0, 0], 1), bucket_key([0xFF, 0, 0], 1));
        // At depth 0 everything shares the single root node.
        assert_eq!(bucket_key([1, 2, 3], 0), bucket_key([250, 10, 200], 0));
    }

    #[test]
    fn accents_from_1x1_image_are_deterministic_and_valid() {
        let path = tmp_path("1x1.png");
        write_png(&path, 1, 1, |_, _| [200, 80, 40, 255]);
        let a = accents_from_image(&path).unwrap();
        let b = accents_from_image(&path).unwrap();
        assert_eq!(a, b, "same image must extract to the same accents");
        for hex in &a {
            assert!(hex.starts_with('#') && hex.len() == 7, "bad hex: {hex}");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn accents_from_fully_flat_image_use_bread_defaults() {
        // A single flat colour has exactly one distinct colour after resize:
        // real wal would fail to reach its own 16-distinct-colour threshold
        // here (it would error out entirely). bread-theme falls back to the
        // curated defaults instead.
        let path = tmp_path("flat.png");
        write_png(&path, 8, 8, |_, _| [120, 120, 120, 255]);
        let accents = accents_from_image(&path).unwrap();
        assert_eq!(accents, crate::palette::default_accents());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn accents_from_fully_transparent_image_use_bread_defaults() {
        let path = tmp_path("transparent.png");
        write_png(&path, 8, 8, |_, _| [200, 30, 30, 0]);
        let accents = accents_from_image(&path).unwrap();
        assert_eq!(accents, crate::palette::default_accents());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn accents_from_unsupported_format_errors() {
        let path = tmp_path("not-an-image.png");
        std::fs::write(&path, b"this is not image data at all, just text").unwrap();
        let result = accents_from_image(&path);
        assert!(result.is_err(), "garbage bytes must not decode");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn accents_from_missing_path_errors() {
        let path = tmp_path("does-not-exist.png");
        let result = accents_from_image(&path);
        assert!(result.is_err());
    }

    #[test]
    fn quantize_like_wal_is_deterministic_across_runs() {
        // A synthetic photo-ish image: smooth two-axis gradient plus a
        // handful of flat accent blocks, comfortably over wal's 16-distinct-
        // colour threshold after resize. Regression guard for determinism
        // (HashMap iteration order must never leak into the result).
        let path = tmp_path("gradient.png");
        write_png(&path, 64, 64, |x, y| {
            [(x * 4) as u8, (y * 4) as u8, ((x + y) * 2) as u8, 255]
        });
        let a = accents_from_image(&path).unwrap();
        let b = accents_from_image(&path).unwrap();
        assert_eq!(a, b);
        for hex in &a {
            assert!(hex.starts_with('#') && hex.len() == 7, "bad hex: {hex}");
        }
        let _ = std::fs::remove_file(&path);
    }
}
