//! Native (no `wal`/ImageMagick subprocess) colour-list extraction from an
//! image: a faithful pure-Rust port of the ImageMagick code paths that
//! decide what `wal -i <img>` produces, so switching from `wal` to this
//! extractor does not change a wallpaper's accents. [`wal_colors`] is the
//! public entry point; everything else here is crate-private. bread-theme's
//! `extract` feature wraps this crate to slice `color1..color6` out of the
//! list and apply bread's own curated fallback when wal itself would have
//! given up; neither of those bread-specific choices lives here.
//!
//! # What pywal does
//!
//! `pywal/backends/wal.py` (the default backend, the only one bread-theme
//! ever shelled out to) runs
//!
//! ```text
//! magick "<img>[0]" -resize 25% -colors N -unique-colors txt:-
//! ```
//!
//! for `N = 16, 17, ..., 35`, stopping at the first `N` whose output has
//! more than 16 lines (a header plus at least 16 colours; if none does, wal
//! exits with an error). Each line is parsed with the regex `#.{6}`, and
//! `color1..color6` end up as entries `8..14` of that list (worked through
//! from `wal.py::adjust`'s `colors[:1] + colors[8:16] + colors[8:-1]`; dark
//! mode never touches those slots afterwards).
//!
//! # What this module ports (ImageMagick 7.1.2, Q16 HDRI)
//!
//! Each stage mirrors one ImageMagick function, including its floating
//! point types and operation order, because the octree and the dither
//! cache are sensitive to the last bit of a value:
//!
//! * `[0]` + decode: JPEGs go through [`crate::jpeg`], a port of
//!   libjpeg-turbo's default decode (ImageMagick's JPEG delegate), so the
//!   samples are identical; other formats (PNG, WebP, GIF, BMP) through
//!   `image`, whose lossless decoders already agree. For GIFs only the first
//!   frame is used, at its own size, with alpha only when it declares a
//!   transparent index, as ImageMagick reads `[0]`. Samples are scaled to
//!   Q16 quantum floats exactly like ImageMagick (`8-bit * 257`, 16-bit as
//!   is). Palette and <=8-bit grayscale PNGs, grayscale JPEGs, GIFs and
//!   palette BMPs are "PseudoClass" in ImageMagick, which changes the
//!   default resize filter (see below).
//! * `-resize 25%`: `geometry.c::ParseMetaGeometry` (`floor(0.25 * size +
//!   0.5)`) and `resize.c::ResizeImage`, [`resize_quarter`]. The default
//!   filter is Lanczos (3-lobe `SincFast`-windowed `SincFast`), or Mitchell
//!   (`CubicBC`, B = C = 1/3) when the image has alpha or is PseudoClass.
//!   Two separable passes, vertical first unless the horizontal factor is
//!   larger, alpha-weighted for colour channels, results stored as `f32`
//!   quantums without clamping (HDRI).
//! * `-colors N`: `quantize.c::QuantizeImage`, [`quantize`]. Grayscale
//!   detection (`IdentifyImageGray`/`SetGrayscaleImage`), tree depth from
//!   `N` (reduced by one for the default Riemersma dither, so depth 3 for
//!   `N` in 16..35, or 8 for gray images), `ClassifyImageColors`
//!   (full-depth classification until the colour count first exceeds `N`,
//!   then `PruneToCubeDepth`), `ReduceImageColors` (`QuantizeErrorFlatten`
//!   threshold, `Reduce`/`PruneChild`), `DefineImageColormap` (mean colour
//!   per node, cast to an `f32` quantum), and `DitherImage`: the Riemersma
//!   (Hilbert curve) error diffusion with its 16-entry exponentially
//!   weighted error queue and its 6-bit-per-channel closest-colour cache,
//!   which decides which colormap entries end up used by at least one
//!   pixel.
//! * `-unique-colors`: `histogram.c::UniqueImageColors`, [`unique_colors`].
//!   Colours are grouped by a 7-level tree whose child id per level is
//!   `red_bit | green_bit << 1 | blue_bit << 2 (| alpha_bit << 3)`: blue is
//!   the most significant, red the least. Within a leaf, colours keep
//!   first-appearance (raster) order.
//! * `txt:-` + pywal's regex: `color.c::GetColorTuple` prints `%02X` per
//!   channel for 8-bit images but `%04X` for 16-bit ones, and pywal's `#.{6}`
//!   keeps only the first six hex digits. For a 16-bit image that is
//!   `RRRRGG` (the red channel's high and low byte, then green's high
//!   byte), not an RGB colour at all. [`hex_like_pywal`] reproduces that
//!   quirk on purpose, because matching what users actually got from pywal
//!   is the point.
//!
//! OpenMP threading in ImageMagick does not affect any of these results:
//! resize computes each output pixel independently, and classification,
//! reduction and the Riemersma dither are sequential.
//!
//! Ported logic is used under the ImageMagick License; see `NOTICE` and
//! `LICENSE-ImageMagick` at the root of this crate.

use image::DynamicImage;
use std::collections::HashMap;
use std::io;
use std::path::Path;

/// `QuantumRange` for a Q16 build.
const QR: f64 = 65535.0;
/// `QuantumScale` (`magick-type.h`): `1.0 / QuantumRange`, in `double`.
const QUANTUM_SCALE: f64 = 1.0 / 65535.0;
/// `MagickEpsilon`.
const EPSILON: f64 = 1.0e-12;

/// wal's own retry range (`wal.py::gen_colors`).
const MIN_TARGET: usize = 16;
const MAX_TARGET: usize = 35;

/// The full ordered list pywal's `gen_colors` returns for `path` (lower-case
/// `#xxxxxx` strings, at least 16 of them), or `None` where wal itself would
/// exit with "Imagemagick couldn't generate a suitable palette". Callers
/// that want `color1..color6` take entries `8..14` of this list, as pywal's
/// own `wal.py::adjust` does (bread-theme's `extract` feature does exactly
/// that; see its `palette_from_image`).
pub fn wal_colors(path: &Path) -> io::Result<Option<Vec<String>>> {
    let src = Source::open(path)?;
    let resized = resize_quarter(&src);
    let gray = is_gray(&resized);
    for n in MIN_TARGET..=MAX_TARGET {
        let quantized = quantize(&resized, n, gray);
        let unique = unique_colors(&quantized);
        if unique.len() >= 16 {
            return Ok(Some(
                unique
                    .iter()
                    .map(|c| hex_like_pywal(c, src.depth16))
                    .collect(),
            ));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Quantum helpers (MagickCore/quantum.h, pixel-accessor.h, statistic-private.h)
// ---------------------------------------------------------------------------

/// `MagickSafeReciprocal`.
fn safe_reciprocal(x: f64) -> f64 {
    if x > -EPSILON && x < EPSILON {
        1.0 / EPSILON
    } else {
        1.0 / x
    }
}

/// `ClampPixel` (HDRI): clamp to `[0, QuantumRange]`, then store as a
/// `float` quantum.
fn clamp_pixel(v: f64) -> f32 {
    if v < 0.0 {
        0.0
    } else if v >= QR {
        QR as f32
    } else {
        v as f32
    }
}

/// `ScaleQuantumToChar` (Q16 HDRI), evaluated in `float` like the C code.
fn scale_to_char(q: f32) -> u8 {
    if q.is_nan() || q <= 0.0 {
        return 0;
    }
    let scaled = q / 257.0f32;
    if scaled >= 255.0f32 {
        255
    } else {
        (scaled + 0.5f32) as u8
    }
}

/// `ScaleQuantumToShort` (Q16 HDRI).
fn scale_to_short(q: f32) -> u16 {
    if q.is_nan() || q <= 0.0 {
        return 0;
    }
    if q >= 65535.0f32 {
        65535
    } else {
        (q + 0.5f32) as u16
    }
}

// ---------------------------------------------------------------------------
// Decode
// ---------------------------------------------------------------------------

enum Samples {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

/// A decoded first frame, as ImageMagick would hold it.
struct Source {
    width: usize,
    height: usize,
    /// 3 (RGB) or 4 (RGBA) interleaved channels.
    channels: usize,
    samples: Samples,
    /// ImageMagick `alpha_trait != UndefinedPixelTrait`.
    alpha: bool,
    /// ImageMagick `storage_class == PseudoClass` right after reading.
    pseudo_class: bool,
    /// Image depth > 8: `txt:` then prints 16-bit hex per channel.
    depth16: bool,
}

impl Source {
    fn open(path: &Path) -> io::Result<Self> {
        let bytes = std::fs::read(path)?;
        let mut truncated = false;
        let mut over_budget = false;
        let decoded = crate::jpeg::decode(&bytes, &mut truncated, &mut over_budget);
        if let Some(d) = decoded {
            // libjpeg-turbo-exact decode; grayscale JPEGs are PseudoClass
            // in ImageMagick.
            let (w, h) = (d.width as u32, d.height as u32);
            let img = if d.channels == 1 {
                image::GrayImage::from_raw(w, h, d.data).map(DynamicImage::ImageLuma8)
            } else {
                image::RgbImage::from_raw(w, h, d.data).map(DynamicImage::ImageRgb8)
            };
            if let Some(img) = img {
                return Ok(Self::from_image(img, d.channels == 1));
            }
        } else if truncated {
            // A genuinely truncated JPEG, not merely an unsupported
            // variant: fail outright rather than let the generic `image`
            // crate fallback silently succeed with its own, different
            // recovery for missing scan data.
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated JPEG: entropy-coded data ended before a scan finished",
            ));
        } else if over_budget {
            // The SOF header declared dimensions over the decode budget.
            // This must be a hard error, not a fall-through to the `image`
            // crate: that crate's `Limits::max_alloc` below only bounds its
            // own decoders, so a JPEG whose header the port already knows
            // is too large would otherwise be handed to `image` anyway,
            // which allocates and decodes the full oversized image before
            // this function ever gets a chance to reject it.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JPEG exceeds the decode budget",
            ));
        }
        // Explicit limits, not `image::load_from_memory`'s undocumented
        // 512 MiB default: that default is too small for a real 16-bit
        // RGBA wallpaper (a 12000x8000 one needs ~732 MiB), and relying on
        // an unstated crate default for a security boundary is fragile.
        // Use the same MAX_DECODE_BUDGET_BYTES ceiling as the JPEG port, so
        // every format this crate decodes answers to one documented cap.
        let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes));
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(crate::jpeg::MAX_DECODE_BUDGET_BYTES);
        reader.limits(limits);
        let mut img = reader
            .with_guessed_format()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
            .decode()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let pseudo_class = sniff_pseudo_class(&bytes);
        if let Some(frame) = gif_first_frame(&bytes) {
            // ImageMagick's "[0]" is the first frame at its own size, and it
            // only has an alpha channel when that frame declares a
            // transparent colour index. `image` returns the frame on a
            // logical-screen-sized RGBA canvas either way.
            if (img.width(), img.height()) != (frame.width, frame.height) {
                img = img.crop_imm(frame.left, frame.top, frame.width, frame.height);
            }
            if !frame.transparent {
                img = DynamicImage::ImageRgb8(img.into_rgb8());
            }
        }
        Ok(Self::from_image(img, pseudo_class))
    }

    fn from_image(img: DynamicImage, pseudo_class: bool) -> Self {
        let (width, height) = (img.width() as usize, img.height() as usize);
        let alpha = img.color().has_alpha();
        let depth16 = img.color().bytes_per_pixel() / img.color().channel_count() > 1;
        let channels = if alpha { 4 } else { 3 };
        let samples = match (depth16, alpha) {
            (false, false) => Samples::U8(img.into_rgb8().into_raw()),
            (false, true) => Samples::U8(img.into_rgba8().into_raw()),
            (true, false) => Samples::U16(img.into_rgb16().into_raw()),
            (true, true) => Samples::U16(img.into_rgba16().into_raw()),
        };
        Self {
            width,
            height,
            channels,
            samples,
            alpha,
            pseudo_class,
            depth16,
        }
    }

    /// Channel `c` of pixel `i` as a Q16 quantum (`ScaleCharToQuantum` is
    /// `257 * v`, exact in `float`).
    #[inline]
    fn get(&self, i: usize, c: usize) -> f64 {
        match &self.samples {
            Samples::U8(v) => (v[i * self.channels + c] as f32 * 257.0f32) as f64,
            Samples::U16(v) => v[i * self.channels + c] as f64,
        }
    }
}

/// Whether ImageMagick's reader would produce a PseudoClass (colormapped)
/// image: palette PNGs and <=8-bit grayscale PNGs, GIFs, and <=8 bpp BMPs.
/// That only matters because `ResizeImage` picks Mitchell instead of
/// Lanczos for PseudoClass images.
fn sniff_pseudo_class(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 26 {
        let bit_depth = bytes[24];
        let color_type = bytes[25];
        return color_type == 3 || (color_type == 0 && bit_depth <= 8);
    }
    if bytes.starts_with(b"GIF8") {
        return true;
    }
    if bytes.starts_with(b"BM") && bytes.len() >= 30 {
        let bpp = u16::from_le_bytes([bytes[28], bytes[29]]);
        return bpp <= 8;
    }
    false
}

struct GifFrame {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    /// The frame's Graphic Control Extension sets a transparent index.
    transparent: bool,
}

/// Geometry and transparency of a GIF's first frame, read from its blocks
/// (`None` for anything that is not a well-formed GIF).
fn gif_first_frame(b: &[u8]) -> Option<GifFrame> {
    if !(b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a")) {
        return None;
    }
    let packed = *b.get(10)?;
    let mut p = 13;
    if packed & 0x80 != 0 {
        p += 3 << ((packed & 7) + 1);
    }
    let mut transparent = false;
    loop {
        match *b.get(p)? {
            0x21 => {
                let label = *b.get(p + 1)?;
                p += 2;
                if label == 0xF9 && *b.get(p)? >= 4 {
                    transparent = b.get(p + 1)? & 1 != 0;
                }
                // Skip the extension's data sub-blocks.
                loop {
                    let n = *b.get(p)? as usize;
                    p += 1 + n;
                    if n == 0 {
                        break;
                    }
                }
            }
            0x2C => {
                let u16_at = |i: usize| -> Option<u32> {
                    Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32)
                };
                return Some(GifFrame {
                    left: u16_at(p + 1)?,
                    top: u16_at(p + 3)?,
                    width: u16_at(p + 5)?,
                    height: u16_at(p + 7)?,
                    transparent,
                });
            }
            _ => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// Resize (MagickCore/resize.c, geometry.c)
// ---------------------------------------------------------------------------

/// A working image: Q16 `float` quantums, always four channels (alpha is
/// `QuantumRange` when the image has none).
#[derive(Clone)]
struct Raster {
    width: usize,
    height: usize,
    alpha: bool,
    px: Vec<[f32; 4]>,
}

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    Lanczos,
    Mitchell,
}

impl Filter {
    fn support(self) -> f64 {
        match self {
            Filter::Lanczos => 3.0,
            Filter::Mitchell => 2.0,
        }
    }

    /// `GetResizeFilterWeight` (blur is 1.0, so `x_blur == |x|`).
    fn weight(self, x: f64) -> f64 {
        let x_blur = x.abs() * safe_reciprocal(1.0);
        match self {
            Filter::Lanczos => {
                // Window scale: filters[Lanczos].scale (1.0) divided by the
                // window support (3.0).
                let scale = 1.0 * safe_reciprocal(3.0);
                let window = sinc_fast(x_blur * scale);
                window * sinc_fast(x_blur)
            }
            Filter::Mitchell => cubic_bc(x_blur),
        }
    }
}

/// `SincFast` for `MAGICKCORE_QUANTUM_DEPTH <= 16`. The coefficients are
/// copied verbatim from resize.c; they round to the same `double`s there.
#[allow(clippy::excessive_precision)]
fn sinc_fast(x: f64) -> f64 {
    if x > 4.0 {
        let alpha = std::f64::consts::PI * x;
        return alpha.sin() / alpha;
    }
    let xx = x * x;
    const C0: f64 = 0.173611107357320220183368594093166520811e-2;
    const C1: f64 = -0.384240921114946632192116762889211361285e-3;
    const C2: f64 = 0.394201182359318128221229891724947048771e-4;
    const C3: f64 = -0.250963301609117217660068889165550534856e-5;
    const C4: f64 = 0.111902032818095784414237782071368805120e-6;
    const C5: f64 = -0.372895101408779549368465614321137048875e-8;
    const C6: f64 = 0.957694196677572570319816780188718518330e-10;
    const C7: f64 = -0.187208577776590710853865174371617338991e-11;
    const C8: f64 = 0.253524321426864752676094495396308636823e-13;
    const C9: f64 = -0.177084805010701112639035485248501049364e-15;
    let p = C0
        + xx * (C1
            + xx * (C2
                + xx * (C3 + xx * (C4 + xx * (C5 + xx * (C6 + xx * (C7 + xx * (C8 + xx * C9))))))));
    (xx - 1.0) * (xx - 4.0) * (xx - 9.0) * (xx - 16.0) * p
}

/// `CubicBC` with Mitchell's B = C = 1/3, coefficients computed exactly as
/// `AcquireResizeFilter` does.
fn cubic_bc(x: f64) -> f64 {
    let b = 1.0 / 3.0;
    let c = 1.0 / 3.0;
    let two_b = b + b;
    let k0 = 1.0 - (1.0 / 3.0) * b;
    let k1 = -3.0 + two_b + c;
    let k2 = 2.0 - 1.5 * b - c;
    let k3 = (4.0 / 3.0) * b + 4.0 * c;
    let k4 = -8.0 * c - two_b;
    let k5 = b + 5.0 * c;
    let k6 = (-1.0 / 6.0) * b - c;
    if x < 1.0 {
        return k0 + x * (x * (k1 + x * k2));
    }
    if x < 2.0 {
        return k3 + x * (k4 + x * (k5 + x * k6));
    }
    0.0
}

/// Per-output-sample contributions for one axis (`HorizontalFilter` /
/// `VerticalFilter`), identical for every row/column.
struct Contribution {
    start: usize,
    weights: Vec<f64>,
}

fn contributions(filter: Filter, factor: f64, in_len: usize, out_len: usize) -> Vec<Contribution> {
    let mut scale = (1.0 / factor + EPSILON).max(1.0);
    let mut support = scale * filter.support();
    if support < 0.5 {
        support = 0.5;
        scale = 1.0;
    }
    scale = safe_reciprocal(scale);
    (0..out_len)
        .map(|o| {
            let bisect = (o as f64 + 0.5) / factor + EPSILON;
            let start = (bisect - support + 0.5).max(0.0) as isize;
            let stop = (bisect + support + 0.5).min(in_len as f64) as isize;
            let mut weights = Vec::with_capacity((stop - start).max(0) as usize);
            let mut density = 0.0;
            for n in 0..(stop - start) {
                let w = filter.weight(scale * ((start + n) as f64 - bisect + 0.5));
                density += w;
                weights.push(w);
            }
            if !weights.is_empty() && density != 0.0 && density != 1.0 {
                let d = safe_reciprocal(density);
                for w in &mut weights {
                    *w *= d;
                }
            }
            Contribution {
                start: start.max(0) as usize,
                weights,
            }
        })
        .collect()
}

/// Apply one filter tap set to `n` samples fetched through `get(j, channel)`.
#[inline]
fn filter_pixel(weights: &[f64], alpha: bool, get: impl Fn(usize, usize) -> f64) -> [f32; 4] {
    let mut out = [QR as f32; 4];
    if !alpha {
        for (c, slot) in out.iter_mut().enumerate().take(3) {
            let mut pixel = 0.0;
            for (j, w) in weights.iter().enumerate() {
                pixel += w * get(j, c);
            }
            *slot = pixel as f32;
        }
        return out;
    }
    // Colour channels are alpha-weighted (BlendPixelTrait); alpha itself is
    // not.
    for (c, slot) in out.iter_mut().enumerate().take(3) {
        let mut pixel = 0.0;
        let mut gamma = 0.0;
        for (j, w) in weights.iter().enumerate() {
            let a = w * QUANTUM_SCALE * get(j, 3);
            pixel += a * get(j, c);
            gamma += a;
        }
        *slot = (safe_reciprocal(gamma) * pixel) as f32;
    }
    let mut pixel = 0.0;
    for (j, w) in weights.iter().enumerate() {
        pixel += w * get(j, 3);
    }
    out[3] = pixel as f32;
    out
}

/// `-resize 25%`: `ParseMetaGeometry` + `ResizeImage` with the default
/// filter.
fn resize_quarter(src: &Source) -> Raster {
    let (w, h) = (src.width, src.height);
    let nw = ((25.0 * w as f64 / 100.0 + 0.5).floor()).max(1.0) as usize;
    let nh = ((25.0 * h as f64 / 100.0 + 0.5).floor()).max(1.0) as usize;
    let alpha = src.alpha;
    let filter = if src.pseudo_class || alpha {
        Filter::Mitchell
    } else {
        Filter::Lanczos
    };
    let x_factor = nw as f64 * safe_reciprocal(w as f64);
    let y_factor = nh as f64 * safe_reciprocal(h as f64);
    let xc = contributions(filter, x_factor, w, nw);
    let yc = contributions(filter, y_factor, h, nh);
    let mut px = vec![[0.0f32; 4]; nw * nh];

    if x_factor > y_factor {
        // Horizontal first into a (nw x h) intermediate, then vertical.
        let mut mid = vec![[0.0f32; 4]; nw * h];
        par_rows(&mut mid, nw, |y, out| {
            for (x, c) in xc.iter().enumerate() {
                if !c.weights.is_empty() {
                    out[x] =
                        filter_pixel(&c.weights, alpha, |j, ch| src.get(y * w + c.start + j, ch));
                }
            }
        });
        par_rows(&mut px, nw, |y, out| {
            let c = &yc[y];
            if c.weights.is_empty() {
                return;
            }
            for (x, slot) in out.iter_mut().enumerate() {
                *slot = filter_pixel(&c.weights, alpha, |j, ch| {
                    mid[(c.start + j) * nw + x][ch] as f64
                });
            }
        });
    } else {
        // Vertical first, one intermediate row per output row (the
        // horizontal pass for output row y only reads intermediate row y).
        par_rows(&mut px, nw, |y, out| {
            let c = &yc[y];
            if c.weights.is_empty() {
                return;
            }
            let row: Vec<[f32; 4]> = (0..w)
                .map(|x| {
                    filter_pixel(&c.weights, alpha, |j, ch| {
                        src.get((c.start + j) * w + x, ch)
                    })
                })
                .collect();
            for (x, cx) in xc.iter().enumerate() {
                if !cx.weights.is_empty() {
                    out[x] = filter_pixel(&cx.weights, alpha, |j, ch| row[cx.start + j][ch] as f64);
                }
            }
        });
    }
    Raster {
        width: nw,
        height: nh,
        alpha,
        px,
    }
}

/// Run `f(row_index, row)` over every `width`-long row of `buf` on all
/// available cores. Every output row is computed independently from
/// read-only inputs (as in ImageMagick's OpenMP loops), so the result does
/// not depend on the thread count.
pub(crate) fn par_rows<T: Send>(buf: &mut [T], width: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let rows = buf.len().checked_div(width).unwrap_or(0);
    if rows == 0 {
        return;
    }
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(rows);
    let per = rows.div_ceil(threads);
    std::thread::scope(|scope| {
        for (i, chunk) in buf.chunks_mut(per * width).enumerate() {
            let f = &f;
            scope.spawn(move || {
                for (k, row) in chunk.chunks_mut(width).enumerate() {
                    f(i * per + k, row);
                }
            });
        }
    });
}

// ---------------------------------------------------------------------------
// Quantize (MagickCore/quantize.c)
// ---------------------------------------------------------------------------

const MAX_TREE_DEPTH: usize = 8;
const ERROR_QUEUE_LENGTH: usize = 16;
const ERROR_RELATIVE_WEIGHT: f64 = 1.0 / 16.0;
const CACHE_SHIFT: u32 = 2;
const NONE: u32 = u32::MAX;

/// `IdentifyImageGray` on the resized image: every pixel has red == green ==
/// blue (to within `MagickEpsilon`).
fn is_gray(r: &Raster) -> bool {
    r.px.iter().all(|p| {
        (p[0] as f64 - p[1] as f64).abs() < EPSILON && (p[1] as f64 - p[2] as f64).abs() < EPSILON
    })
}

#[derive(Clone)]
struct Node {
    parent: u32,
    child: [u32; 16],
    number_unique: u64,
    total: [f64; 4],
    quantize_error: f64,
    color_number: usize,
    id: usize,
    level: usize,
}

struct Cube {
    nodes: Vec<Node>,
    /// `cube_info->nodes`: live node count (allocated minus merged).
    live: usize,
    colors: usize,
    maximum_colors: usize,
    depth: usize,
    associate_alpha: bool,
    pruning_threshold: f64,
    next_threshold: f64,
    colormap: Vec<[f64; 4]>,
    // Dither state.
    target: [f64; 4],
    distance: f64,
    color_number: usize,
}

const ROOT: u32 = 0;

impl Cube {
    fn new(depth: usize, maximum_colors: usize, associate_alpha: bool) -> Self {
        let mut cube = Cube {
            nodes: Vec::with_capacity(4096),
            live: 0,
            colors: 0,
            maximum_colors,
            depth: depth.clamp(2, MAX_TREE_DEPTH),
            associate_alpha,
            pruning_threshold: 0.0,
            next_threshold: 0.0,
            colormap: Vec::new(),
            target: [0.0; 4],
            distance: 0.0,
            color_number: 0,
        };
        cube.new_node(0, 0, ROOT);
        cube
    }

    fn children(&self) -> usize {
        if self.associate_alpha {
            16
        } else {
            8
        }
    }

    fn new_node(&mut self, id: usize, level: usize, parent: u32) -> u32 {
        self.live += 1;
        self.nodes.push(Node {
            parent,
            child: [NONE; 16],
            number_unique: 0,
            total: [0.0; 4],
            quantize_error: 0.0,
            color_number: 0,
            id,
            level,
        });
        (self.nodes.len() - 1) as u32
    }

    /// `ColorToQNodeId`.
    fn node_id(&self, p: &[f64; 4], index: usize) -> usize {
        let bit = |v: f64| ((scale_to_char(clamp_pixel(v)) >> index) & 1) as usize;
        let mut id = bit(p[0]) | bit(p[1]) << 1 | bit(p[2]) << 2;
        if self.associate_alpha {
            id |= bit(p[3]) << 3;
        }
        id
    }

    /// `AssociateAlphaPixel`.
    fn associate(&self, p: &[f32; 4]) -> [f64; 4] {
        let a = p[3] as f64;
        if !self.associate_alpha || p[3] == QR as f32 {
            return [p[0] as f64, p[1] as f64, p[2] as f64, a];
        }
        let alpha = QUANTUM_SCALE * a;
        [
            alpha * p[0] as f64,
            alpha * p[1] as f64,
            alpha * p[2] as f64,
            a,
        ]
    }

    /// `AssociateAlphaPixelInfo` on a colormap entry.
    fn associate_info(&self, p: &[f64; 4]) -> [f64; 4] {
        if !self.associate_alpha || p[3] == QR {
            return *p;
        }
        let alpha = QUANTUM_SCALE * p[3];
        [alpha * p[0], alpha * p[1], alpha * p[2], p[3]]
    }

    /// The per-run body of `ClassifyImageColors` (both of its loops).
    fn classify(&mut self, pixel: &[f64; 4], count: usize, max_level: usize) {
        let midpoint = QR / 2.0;
        let mut mid = [midpoint; 4];
        let mut bisect = (QR + 1.0) / 2.0;
        let mut index = MAX_TREE_DEPTH - 1;
        let mut node = ROOT;
        let countf = count as f64;
        for level in 1..=max_level {
            bisect *= 0.5;
            let id = self.node_id(pixel, index);
            mid[0] += if id & 1 != 0 { bisect } else { -bisect };
            mid[1] += if id & 2 != 0 { bisect } else { -bisect };
            mid[2] += if id & 4 != 0 { bisect } else { -bisect };
            mid[3] += if id & 8 != 0 { bisect } else { -bisect };
            if self.nodes[node as usize].child[id] == NONE {
                let child = self.new_node(id, level, node);
                self.nodes[node as usize].child[id] = child;
                if level == max_level {
                    self.colors += 1;
                }
            }
            node = self.nodes[node as usize].child[id];
            let er = QUANTUM_SCALE * (pixel[0] - mid[0]);
            let eg = QUANTUM_SCALE * (pixel[1] - mid[1]);
            let eb = QUANTUM_SCALE * (pixel[2] - mid[2]);
            let ea = if self.associate_alpha {
                QUANTUM_SCALE * (pixel[3] - mid[3])
            } else {
                0.0
            };
            let mut distance = er * er + eg * eg + eb * eb + ea * ea;
            if distance.is_nan() {
                distance = 0.0;
            }
            self.nodes[node as usize].quantize_error += countf * distance.sqrt();
            let qe = self.nodes[node as usize].quantize_error;
            self.nodes[ROOT as usize].quantize_error += qe;
            index = index.wrapping_sub(1);
        }
        let alpha_total = if self.associate_alpha {
            clamp_pixel(pixel[3])
        } else {
            clamp_pixel(QR)
        };
        let n = &mut self.nodes[node as usize];
        n.number_unique += count as u64;
        n.total[0] += countf * QUANTUM_SCALE * clamp_pixel(pixel[0]) as f64;
        n.total[1] += countf * QUANTUM_SCALE * clamp_pixel(pixel[1]) as f64;
        n.total[2] += countf * QUANTUM_SCALE * clamp_pixel(pixel[2]) as f64;
        n.total[3] += countf * QUANTUM_SCALE * alpha_total as f64;
    }

    /// `PruneChild`.
    fn prune_child(&mut self, node: u32) {
        for i in 0..self.children() {
            let c = self.nodes[node as usize].child[i];
            if c != NONE {
                self.prune_child(c);
            }
        }
        if self.live > self.maximum_colors {
            let n = self.nodes[node as usize].clone();
            let p = &mut self.nodes[n.parent as usize];
            p.number_unique += n.number_unique;
            for c in 0..4 {
                p.total[c] += n.total[c];
            }
            p.child[n.id] = NONE;
            self.live -= 1;
        }
    }

    /// `PruneLevel`.
    fn prune_level(&mut self, node: u32) {
        for i in 0..self.children() {
            let c = self.nodes[node as usize].child[i];
            if c != NONE {
                self.prune_level(c);
            }
        }
        if self.nodes[node as usize].level == self.depth {
            self.prune_child(node);
        }
    }

    /// `PruneToCubeDepth`.
    fn prune_to_cube_depth(&mut self, node: u32) {
        for i in 0..self.children() {
            let c = self.nodes[node as usize].child[i];
            if c != NONE {
                self.prune_to_cube_depth(c);
            }
        }
        if self.nodes[node as usize].level > self.depth {
            self.prune_child(node);
        }
    }

    /// `QuantizeErrorFlatten`.
    fn flatten_errors(&self, node: u32, offset: usize, out: &mut Vec<f64>) -> usize {
        if offset >= self.live {
            return 0;
        }
        out.push(self.nodes[node as usize].quantize_error);
        let mut n = 1;
        for i in 0..self.children() {
            let c = self.nodes[node as usize].child[i];
            if c != NONE {
                n += self.flatten_errors(c, offset + n, out);
            }
        }
        n
    }

    /// `Reduce`.
    fn reduce(&mut self, node: u32) {
        for i in 0..self.children() {
            let c = self.nodes[node as usize].child[i];
            if c != NONE {
                self.reduce(c);
            }
        }
        let n = &self.nodes[node as usize];
        if n.quantize_error <= self.pruning_threshold {
            self.prune_child(node);
        } else {
            if n.number_unique > 0 {
                self.colors += 1;
            }
            if n.quantize_error < self.next_threshold {
                self.next_threshold = n.quantize_error;
            }
        }
    }

    /// `ReduceImageColors`.
    fn reduce_colors(&mut self) {
        self.next_threshold = 0.0;
        if self.colors > self.maximum_colors {
            let mut errors = Vec::with_capacity(self.live);
            self.flatten_errors(ROOT, 0, &mut errors);
            errors.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let keep = 110 * (self.maximum_colors + 1) / 100;
            if self.live > keep {
                self.next_threshold = errors[self.live - keep];
            }
        }
        while self.colors > self.maximum_colors {
            self.pruning_threshold = self.next_threshold;
            self.next_threshold = self.nodes[ROOT as usize].quantize_error - 1.0;
            self.colors = 0;
            self.reduce(ROOT);
        }
    }

    /// `DefineImageColormap`.
    fn define_colormap(&mut self, node: u32) {
        for i in 0..self.children() {
            let c = self.nodes[node as usize].child[i];
            if c != NONE {
                self.define_colormap(c);
            }
        }
        let n = &self.nodes[node as usize];
        if n.number_unique == 0 {
            return;
        }
        let alpha = safe_reciprocal(n.number_unique as i64 as f64);
        let t = n.total;
        let entry = if !self.associate_alpha {
            [
                (alpha * QR * t[0]) as f32 as f64,
                (alpha * QR * t[1]) as f32 as f64,
                (alpha * QR * t[2]) as f32 as f64,
                QR,
            ]
        } else {
            let opacity = alpha * QR * t[3];
            let a = opacity as f32 as f64;
            if a == QR {
                [
                    (alpha * QR * t[0]) as f32 as f64,
                    (alpha * QR * t[1]) as f32 as f64,
                    (alpha * QR * t[2]) as f32 as f64,
                    a,
                ]
            } else {
                let gamma = safe_reciprocal(QUANTUM_SCALE * a);
                [
                    (alpha * gamma * QR * t[0]) as f32 as f64,
                    (alpha * gamma * QR * t[1]) as f32 as f64,
                    (alpha * gamma * QR * t[2]) as f32 as f64,
                    a,
                ]
            }
        };
        self.nodes[node as usize].color_number = self.colormap.len();
        self.colormap.push(entry);
    }

    /// `ClosestColor`.
    fn closest_color(&mut self, node: u32) {
        for i in 0..self.children() {
            let c = self.nodes[node as usize].child[i];
            if c != NONE {
                self.closest_color(c);
            }
        }
        let n = &self.nodes[node as usize];
        if n.number_unique == 0 {
            return;
        }
        let p = self.colormap[n.color_number];
        let q = self.target;
        let (alpha, beta) = if self.associate_alpha {
            (QUANTUM_SCALE * p[3], QUANTUM_SCALE * q[3])
        } else {
            (1.0, 1.0)
        };
        let mut pixel = alpha * p[0] - beta * q[0];
        let mut distance = pixel * pixel;
        if distance <= self.distance {
            pixel = alpha * p[1] - beta * q[1];
            distance += pixel * pixel;
            if distance <= self.distance {
                pixel = alpha * p[2] - beta * q[2];
                distance += pixel * pixel;
                if distance <= self.distance {
                    if self.associate_alpha {
                        pixel = p[3] - q[3];
                        distance += pixel * pixel;
                    }
                    if distance <= self.distance {
                        self.distance = distance;
                        self.color_number = n.color_number;
                    }
                }
            }
        }
    }

    /// `CacheOffset`.
    fn cache_offset(&self, p: &[f64; 4]) -> usize {
        let s = |v: f64| (scale_to_char(clamp_pixel(v)) >> CACHE_SHIFT) as usize;
        let mut offset = s(p[0]) | s(p[1]) << 6 | s(p[2]) << 12;
        if self.associate_alpha {
            offset |= s(p[3]) << 18;
        }
        offset
    }
}

/// `IsPixelEquivalent(image, p, q)`, both pixels from the same image.
fn pixel_equivalent(p: &[f32; 4], q: &[f32; 4], alpha: bool) -> bool {
    let (a, b) = if alpha {
        (p[3] as f64, q[3] as f64)
    } else {
        (QR, QR)
    };
    if (a - b).abs() >= EPSILON {
        return false;
    }
    if a.abs() < EPSILON || b.abs() < EPSILON {
        return true;
    }
    (0..3).all(|c| (p[c] as f64 - q[c] as f64).abs() < EPSILON)
}

/// `QuantizeImage` with pywal's settings (`-colors n`, default Riemersma
/// dither, no `-treedepth`, no `-colorspace`). Returns the quantized image.
fn quantize(input: &Raster, n: usize, gray: bool) -> Raster {
    let mut image = input.clone();
    if gray {
        // SetGrayscaleImage: sRGB -> GRAY is an identity for r == g == b up
        // to float rounding; applied literally for exactness.
        for p in &mut image.px {
            let g =
                (0.212656 * p[0] as f64 + 0.715158 * p[1] as f64 + 0.072186 * p[2] as f64) as f32;
            p[0] = g;
            p[1] = g;
            p[2] = g;
        }
    }
    let mut depth = 1;
    let mut colors = n;
    while colors != 0 {
        colors >>= 2;
        depth += 1;
    }
    // Riemersma dithering is on by default.
    if depth > 2 {
        depth -= 1;
    }
    if image.alpha && depth > 5 {
        depth -= 1;
    }
    if gray {
        depth = MAX_TREE_DEPTH;
    }
    let mut cube = Cube::new(depth, n, image.alpha);
    classify_image(&mut cube, &image);
    if cube.colors > cube.maximum_colors {
        cube.reduce_colors();
    }
    cube.define_colormap(ROOT);
    riemersma_dither(&mut cube, &mut image);
    image
}

/// `ClassifyImageColors`.
fn classify_image(cube: &mut Cube, image: &Raster) {
    /// `MaxQNodes`.
    const MAX_QNODES: usize = 266817;
    let (w, h) = (image.width, image.height);
    let mut y = 0;
    let mut phase_one = true;
    while y < h {
        if cube.live > MAX_QNODES {
            cube.prune_level(ROOT);
            cube.depth -= 1;
        }
        let row = &image.px[y * w..(y + 1) * w];
        let max_level = if phase_one {
            MAX_TREE_DEPTH
        } else {
            cube.depth
        };
        let mut x = 0;
        while x < w {
            let mut count = 1;
            while x + count < w && pixel_equivalent(&row[x], &row[x + count], image.alpha) {
                count += 1;
            }
            let pixel = cube.associate(&row[x]);
            cube.classify(&pixel, count, max_level);
            x += count;
        }
        if phase_one && cube.colors > cube.maximum_colors {
            cube.prune_to_cube_depth(ROOT);
            phase_one = false;
        }
        y += 1;
    }
}

/// `DitherImage` (Riemersma) followed by `SyncImage`.
fn riemersma_dither(cube: &mut Cube, image: &mut Raster) {
    let mut weights = [0.0f64; ERROR_QUEUE_LENGTH];
    let mut weight = 1.0f64;
    for w in weights.iter_mut() {
        *w = safe_reciprocal(weight);
        weight *= ((1.0 / ERROR_RELATIVE_WEIGHT).ln() / (ERROR_QUEUE_LENGTH as f64 - 1.0)).exp();
    }
    let cache_len = 1usize << (if cube.associate_alpha { 4 } else { 3 } * (8 - CACHE_SHIFT));
    let mut state = Dither {
        x: 0,
        y: 0,
        error: [[0.0; 4]; ERROR_QUEUE_LENGTH],
        weights,
        cache: vec![u8::MAX; cache_len],
    };
    let extent = image.width.max(image.height);
    let mut level = (extent as f64).log2() as usize;
    if (1usize << level) < extent {
        level += 1;
    }
    if level > 0 {
        state.hilbert(cube, image, level, Dir::North);
    }
    state.step(cube, image, Dir::Forget);
}

#[derive(Clone, Copy)]
enum Dir {
    West,
    East,
    North,
    South,
    Forget,
}

struct Dither {
    x: isize,
    y: isize,
    error: [[f64; 4]; ERROR_QUEUE_LENGTH],
    weights: [f64; ERROR_QUEUE_LENGTH],
    /// Colour index per `CacheOffset`; `u8::MAX` means "not computed" (at
    /// most 35 colours, so an index never reaches it).
    cache: Vec<u8>,
}

impl Dither {
    /// `Riemersma`.
    fn hilbert(&mut self, cube: &mut Cube, image: &mut Raster, level: usize, dir: Dir) {
        use Dir::*;
        if level == 1 {
            let seq: [Dir; 3] = match dir {
                West => [East, South, West],
                East => [West, North, East],
                North => [South, East, North],
                South => [North, West, South],
                Forget => return,
            };
            for d in seq {
                self.step(cube, image, d);
            }
            return;
        }
        let (a, b, c, d, e, f, g) = match dir {
            West => (North, East, West, South, West, West, South),
            East => (South, West, East, North, East, East, North),
            North => (West, South, North, East, North, North, East),
            South => (East, North, South, West, South, South, West),
            Forget => return,
        };
        self.hilbert(cube, image, level - 1, a);
        self.step(cube, image, b);
        self.hilbert(cube, image, level - 1, c);
        self.step(cube, image, d);
        self.hilbert(cube, image, level - 1, e);
        self.step(cube, image, f);
        self.hilbert(cube, image, level - 1, g);
    }

    /// `RiemersmaDither`.
    fn step(&mut self, cube: &mut Cube, image: &mut Raster, dir: Dir) {
        if self.x >= 0
            && (self.x as usize) < image.width
            && self.y >= 0
            && (self.y as usize) < image.height
        {
            let idx = self.y as usize * image.width + self.x as usize;
            let mut pixel = cube.associate(&image.px[idx]);
            for i in 0..ERROR_QUEUE_LENGTH {
                let k = ERROR_RELATIVE_WEIGHT * 1.0 * self.weights[i];
                pixel[0] += k * self.error[i][0];
                pixel[1] += k * self.error[i][1];
                pixel[2] += k * self.error[i][2];
                if cube.associate_alpha {
                    pixel[3] += k * self.error[i][3];
                }
            }
            pixel[0] = clamp_pixel(pixel[0]) as f64;
            pixel[1] = clamp_pixel(pixel[1]) as f64;
            pixel[2] = clamp_pixel(pixel[2]) as f64;
            if cube.associate_alpha {
                pixel[3] = clamp_pixel(pixel[3]) as f64;
            }
            let offset = cube.cache_offset(&pixel);
            if self.cache[offset] == u8::MAX {
                let mut node = ROOT;
                let mut index = MAX_TREE_DEPTH - 1;
                while index > 0 {
                    let id = cube.node_id(&pixel, index);
                    let c = cube.nodes[node as usize].child[id];
                    if c == NONE {
                        break;
                    }
                    node = c;
                    index -= 1;
                }
                cube.target = pixel;
                cube.distance = 4.0 * (QR + 1.0) * (QR + 1.0) + 1.0;
                let parent = cube.nodes[node as usize].parent;
                cube.closest_color(parent);
                self.cache[offset] = cube.color_number as u8;
            }
            let index = self.cache[offset] as usize;
            let entry = cube.colormap[index];
            let q = &mut image.px[idx];
            q[0] = entry[0] as f32;
            q[1] = entry[1] as f32;
            q[2] = entry[2] as f32;
            if cube.associate_alpha {
                q[3] = entry[3] as f32;
            }
            self.error.copy_within(1.., 0);
            let color = cube.associate_info(&entry);
            let last = &mut self.error[ERROR_QUEUE_LENGTH - 1];
            last[0] = pixel[0] - color[0];
            last[1] = pixel[1] - color[1];
            last[2] = pixel[2] - color[2];
            if cube.associate_alpha {
                last[3] = pixel[3] - color[3];
            }
        }
        match dir {
            Dir::West => self.x -= 1,
            Dir::East => self.x += 1,
            Dir::North => self.y -= 1,
            Dir::South => self.y += 1,
            Dir::Forget => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Unique colours (MagickCore/histogram.c) and pywal's txt parsing
// ---------------------------------------------------------------------------

/// `UniqueImageColors`: distinct pixel colours in histogram-tree order.
fn unique_colors(image: &Raster) -> Vec<[f32; 4]> {
    // Tree path key: 7 levels (bits 7..1 of each 8-bit-scaled channel), 4
    // bits per level, level 1 most significant. Child ids are visited in
    // ascending order, so sorting by this key walks the tree in the same
    // order `UniqueColorsToImage` does.
    let key_of = |p: &[f32; 4]| -> u32 {
        let mut key = 0u32;
        for index in (1..MAX_TREE_DEPTH).rev() {
            let bit = |v: f32| ((scale_to_char(v) >> index) & 1) as u32;
            let mut id = bit(p[0]) | bit(p[1]) << 1 | bit(p[2]) << 2;
            if image.alpha {
                id |= bit(p[3]) << 3;
            }
            key = key << 4 | id;
        }
        key
    };
    let norm = |v: f32| if v == 0.0 { 0u32 } else { v.to_bits() };
    let mut seen: HashMap<[u32; 4], ()> = HashMap::new();
    let mut list: Vec<(u32, [f32; 4])> = Vec::new();
    for p in &image.px {
        let a = if image.alpha { p[3] } else { QR as f32 };
        let bits = [norm(p[0]), norm(p[1]), norm(p[2]), norm(a)];
        if seen.insert(bits, ()).is_none() {
            list.push((key_of(p), *p));
        }
    }
    // Stable: equal keys (same leaf) keep first-appearance order.
    list.sort_by_key(|(k, _)| *k);
    list.into_iter().map(|(_, p)| p).collect()
}

/// One `txt:` colour as pywal's `re.search("#.{6}", line)` sees it,
/// lower-cased. 8-bit images print `#RRGGBB`; 16-bit images print
/// `#RRRRGGGGBBBB`, which pywal truncates to `#RRRRGG`.
fn hex_like_pywal(p: &[f32; 4], depth16: bool) -> String {
    if depth16 {
        let full = format!(
            "{:04x}{:04x}{:04x}",
            scale_to_short(p[0]),
            scale_to_short(p[1]),
            scale_to_short(p[2])
        );
        format!("#{}", &full[..6])
    } else {
        format!(
            "#{:02x}{:02x}{:02x}",
            scale_to_char(p[0]),
            scale_to_char(p[1]),
            scale_to_char(p[2])
        )
    }
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

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn quantum_scaling_matches_imagemagick_q16_hdri() {
        // ScaleCharToQuantum(v) = 257 v round-trips exactly.
        for v in 0..=255u8 {
            assert_eq!(scale_to_char(v as f32 * 257.0), v);
        }
        // Rounding is float `(q / 257) + 0.5` truncated.
        assert_eq!(scale_to_char(128.49 * 257.0), 128);
        assert_eq!(scale_to_char(128.5 * 257.0), 129);
        assert_eq!(scale_to_char(-3.0), 0);
        assert_eq!(scale_to_char(70000.0), 255);
        assert_eq!(scale_to_short(1234.5), 1235);
        assert_eq!(clamp_pixel(-1.0), 0.0);
        assert_eq!(clamp_pixel(70000.0), 65535.0);
    }

    #[test]
    fn resize_geometry_rounds_like_parse_meta_geometry() {
        let src = |w, h| Source {
            width: w,
            height: h,
            channels: 3,
            samples: Samples::U8(vec![0; w * h * 3]),
            alpha: false,
            pseudo_class: false,
            depth16: false,
        };
        // floor(0.25 * n + 0.5), at least 1.
        let r = resize_quarter(&src(1201, 6));
        assert_eq!((r.width, r.height), (300, 2));
        let r = resize_quarter(&src(1, 2));
        assert_eq!((r.width, r.height), (1, 1));
        let r = resize_quarter(&src(1922, 1920));
        assert_eq!((r.width, r.height), (481, 480));
    }

    #[test]
    fn lanczos_and_mitchell_weights_match_imagemagick() {
        // Both kernels are 1 at the centre and 0 at integer offsets (Lanczos)
        // or at the support edge (Mitchell), and SincFast agrees with the
        // real sinc to the 1/2^25 precision its Q16 coefficients promise.
        assert!((Filter::Lanczos.weight(0.0) - 1.0).abs() < 1e-7);
        assert!(Filter::Lanczos.weight(1.0).abs() < 1e-7);
        assert!(Filter::Lanczos.weight(2.0).abs() < 1e-7);
        for i in 1..40 {
            let x = i as f64 / 10.0;
            let exact = (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x);
            assert!((sinc_fast(x) - exact).abs() < 1e-7, "sinc_fast({x})");
        }
        assert!((Filter::Mitchell.weight(0.0) - 8.0 / 9.0).abs() < 1e-12);
        assert_eq!(Filter::Mitchell.weight(2.0), 0.0);
    }

    #[test]
    fn unique_colors_order_blue_most_significant_red_least() {
        // ImageMagick's histogram child id is r | g << 1 | b << 2, so at the
        // top level pure red (id 1) sorts before pure green (2), which
        // sorts before pure blue (4). A red-most-significant order would
        // put red last.
        let q = |r: u8, g: u8, b: u8| {
            [
                r as f32 * 257.0,
                g as f32 * 257.0,
                b as f32 * 257.0,
                65535.0,
            ]
        };
        let raster = Raster {
            width: 4,
            height: 1,
            alpha: false,
            px: vec![q(0, 0, 255), q(0, 255, 0), q(255, 0, 0), q(0, 0, 0)],
        };
        let order: Vec<String> = unique_colors(&raster)
            .iter()
            .map(|c| hex_like_pywal(c, false))
            .collect();
        assert_eq!(order, ["#000000", "#ff0000", "#00ff00", "#0000ff"]);
    }

    #[test]
    fn unique_colors_keep_first_appearance_within_a_leaf() {
        // 0x80 and 0x81 differ only in bit 0, which the 7-level histogram
        // tree ignores: same leaf, so raster order decides.
        let q = |v: u8| [v as f32 * 257.0, 0.0, 0.0, 65535.0];
        let raster = Raster {
            width: 3,
            height: 1,
            alpha: false,
            px: vec![q(0x81), q(0x80), q(0x81)],
        };
        let order: Vec<String> = unique_colors(&raster)
            .iter()
            .map(|c| hex_like_pywal(c, false))
            .collect();
        assert_eq!(order, ["#810000", "#800000"]);
    }

    #[test]
    fn sixteen_bit_hex_is_truncated_like_pywals_regex() {
        // txt: prints #RRRRGGGGBBBB for 16-bit images and pywal's `#.{6}`
        // keeps "#RRRRGG".
        let c = [0x78F0 as f32, 0x1234 as f32, 0xABCD as f32, 65535.0];
        assert_eq!(hex_like_pywal(&c, true), "#78f012");
        let c8 = [
            0x78 as f32 * 257.0,
            0x12 as f32 * 257.0,
            0xAB as f32 * 257.0,
            65535.0,
        ];
        assert_eq!(hex_like_pywal(&c8, false), "#7812ab");
    }

    #[test]
    fn pseudo_class_sniffing() {
        let png = |bit_depth: u8, color_type: u8| {
            let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01".to_vec();
            b.push(bit_depth);
            b.push(color_type);
            b
        };
        assert!(sniff_pseudo_class(&png(8, 3)));
        assert!(sniff_pseudo_class(&png(8, 0)));
        assert!(!sniff_pseudo_class(&png(16, 0)));
        assert!(!sniff_pseudo_class(&png(8, 2)));
        assert!(!sniff_pseudo_class(&png(8, 6)));
        assert!(sniff_pseudo_class(b"GIF89a"));
        assert!(!sniff_pseudo_class(b"\xFF\xD8\xFF"));
    }

    /// `color1..color6` are entries `8..14` of [`wal_colors`]' list, exactly
    /// as `bread_theme`'s `extract` feature slices them; tests here that
    /// care about the six accent slots rather than the full list use this
    /// so they read the same as the callers they stand in for.
    fn accents(path: &std::path::Path) -> io::Result<[String; 6]> {
        let colors = wal_colors(path)?
            .unwrap_or_else(|| panic!("{path:?}: wal would have given up (fewer than 16 colours)"));
        Ok(std::array::from_fn(|i| colors[8 + i].clone()))
    }

    #[test]
    fn wal_colors_are_deterministic() {
        let a = wal_colors(&fixture("bos-default.jpg")).unwrap();
        for _ in 0..3 {
            assert_eq!(wal_colors(&fixture("bos-default.jpg")).unwrap(), a);
        }
    }

    #[test]
    fn real_photo_accents_are_never_near_black() {
        // Regression guard for the old octree fallthrough that returned the
        // darkest raw colours of an unquantized list.
        for name in [
            "bos-default.jpg",
            "bos-default.png",
            "bos-default.webp",
            "dark-moody.png",
        ] {
            let accents = accents(&fixture(name)).unwrap();
            let bright = accents
                .iter()
                .filter(|h| {
                    let v = u32::from_str_radix(&h[1..], 16).unwrap();
                    (v >> 16) & 0xff >= 16 || (v >> 8) & 0xff >= 16 || v & 0xff >= 16
                })
                .count();
            assert_eq!(bright, 6, "{name}: near-black accents {accents:?}");
        }
    }

    #[test]
    fn wal_colors_is_none_when_wal_would_give_up() {
        // One pixel, a fully flat image, and a fully transparent image are
        // all cases where real wal's ImageMagick pipeline never reaches 16
        // unique colours for any `N` in its retry range, so it exits with
        // "Imagemagick couldn't generate a suitable palette". Bread's own
        // curated fallback for this case lives in bread-theme's `extract`
        // feature (`accents_from_image`), not here: this crate's contract
        // is just to report the same "wal gave up" outcome wal itself would.
        let path = tmp_path("1x1.png");
        write_png(&path, 1, 1, |_, _| [200, 80, 40, 255]);
        assert_eq!(wal_colors(&path).unwrap(), None);
        let _ = std::fs::remove_file(&path);

        let path = tmp_path("flat.png");
        write_png(&path, 64, 64, |_, _| [120, 120, 120, 255]);
        assert_eq!(wal_colors(&path).unwrap(), None);
        let _ = std::fs::remove_file(&path);

        let path = tmp_path("transparent.png");
        write_png(&path, 8, 8, |_, _| [200, 30, 30, 0]);
        assert_eq!(wal_colors(&path).unwrap(), None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn undecodable_input_is_an_error() {
        let path = tmp_path("not-an-image.png");
        std::fs::write(&path, b"this is not image data at all, just text").unwrap();
        assert!(wal_colors(&path).is_err(), "garbage must not decode");
        let _ = std::fs::remove_file(&path);

        // A JPEG cut off inside its headers: neither decoder can use it.
        let path = tmp_path("truncated.jpg");
        let bytes = std::fs::read(fixture("bos-default.jpg")).unwrap();
        std::fs::write(&path, &bytes[..40]).unwrap();
        assert!(wal_colors(&path).is_err(), "truncated JPEG must not decode");
        let _ = std::fs::remove_file(&path);

        // Cut well past the headers, mid scan (bos-default.jpg is a
        // single-scan baseline JPEG, so a mid-file cut deterministically
        // lands inside its one scan's entropy data): the ported decoder
        // used to return `Ok` here with wrong colours (zero-filled past
        // the cut), matching neither wal nor an honest failure. It now
        // reports the cut via `jpeg::decode`'s `truncated_out` flag, and
        // `Source::open` turns that into an explicit `UnexpectedEof`
        // instead of trying the generic `image` crate fallback.
        let path = tmp_path("truncated-midscan.jpg");
        std::fs::write(&path, &bytes[..bytes.len() * 7 / 10]).unwrap();
        let err = wal_colors(&path).expect_err("JPEG truncated mid-scan must not decode");
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        let _ = std::fs::remove_file(&path);

        assert!(wal_colors(&tmp_path("does-not-exist.png")).is_err());
    }

    /// Overwrite an SOF0 segment's declared height/width (big-endian, right
    /// after the 1-byte precision field) in an otherwise real, complete
    /// JPEG. Used to check the budget rejection at the `wal_colors` level,
    /// with real scan data behind the lie, not just a truncated header.
    fn patch_sof_dimensions(data: &[u8], width: u16, height: u16) -> Vec<u8> {
        let mut out = data.to_vec();
        let mut pos = 2;
        while pos + 4 <= out.len() {
            if out[pos] != 0xFF {
                pos += 1;
                continue;
            }
            let marker = out[pos + 1];
            if marker == 0xFF {
                pos += 1;
                continue;
            }
            if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
                pos += 2;
                continue;
            }
            if marker == 0xD8 {
                pos += 2;
                continue;
            }
            if marker == 0xD9 {
                break;
            }
            let len = ((out[pos + 2] as usize) << 8) | out[pos + 3] as usize;
            if (0xC0..=0xC3).contains(&marker) {
                let seg = pos + 4; // past marker + length
                out[seg + 1] = (height >> 8) as u8;
                out[seg + 2] = (height & 0xFF) as u8;
                out[seg + 3] = (width >> 8) as u8;
                out[seg + 4] = (width & 0xFF) as u8;
                return out;
            }
            pos += 2 + len;
        }
        panic!("no SOF marker found in fixture");
    }

    /// A real, complete baseline JPEG with only its SOF0 dimensions lied
    /// about (20000x20000, comfortably over budget but not the extreme
    /// 65500x65500 [`crate::jpeg`]'s own header-only test uses): this must
    /// be rejected by `wal_colors` itself, quickly and without decoding.
    /// The port's own unit test only asserts `decode(..).is_none()`, which
    /// would also pass if a lazily-zeroed allocation had happened first and
    /// merely panicked later; asserting `Err` here, at the public API, with
    /// a real scan behind the lie, is what actually pins "never falls
    /// through to the `image` crate and decodes the same oversized claim a
    /// second time".
    #[test]
    fn over_budget_jpeg_is_rejected_quickly_not_fallen_back_to() {
        let bytes = std::fs::read(fixture("bos-default.jpg")).unwrap();
        let patched = patch_sof_dimensions(&bytes, 20000, 20000);
        let path = tmp_path("over-budget.jpg");
        std::fs::write(&path, &patched).unwrap();

        let start = std::time::Instant::now();
        let err = wal_colors(&path).expect_err("over-budget JPEG must not decode");
        let elapsed = start.elapsed();

        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "rejection took {elapsed:?}, looks like it fell through and actually decoded"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A crafted PNG whose IHDR alone declares 65500x65500 (RGBA, 16-bit):
    /// the `image` crate's own decoder validates the declared size against
    /// `MAX_DECODE_BUDGET_BYTES` before allocating the output buffer, the
    /// same protection [`crate::jpeg`]'s allocation-cap test exercises for
    /// the ported JPEG path. This never allocates the (tens of gigabytes)
    /// buffer the header claims.
    #[test]
    fn huge_png_header_is_rejected_before_allocating() {
        // Minimal PNG: signature + IHDR (65500x65500, 16-bit RGBA) + an
        // otherwise-empty IDAT/IEND, enough for the decoder to read the
        // header and reject it on size before it ever needs real pixel
        // data.
        fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            let mut crc_input = Vec::new();
            crc_input.extend_from_slice(kind);
            crc_input.extend_from_slice(data);
            out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
            out
        }
        fn crc32(data: &[u8]) -> u32 {
            let mut table = [0u32; 256];
            for (i, entry) in table.iter_mut().enumerate() {
                let mut c = i as u32;
                for _ in 0..8 {
                    c = if c & 1 != 0 {
                        0xEDB88320 ^ (c >> 1)
                    } else {
                        c >> 1
                    };
                }
                *entry = c;
            }
            let mut crc = 0xFFFFFFFFu32;
            for &b in data {
                crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
            }
            crc ^ 0xFFFFFFFF
        }
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&65500u32.to_be_bytes()); // width
        ihdr.extend_from_slice(&65500u32.to_be_bytes()); // height
        ihdr.push(16); // bit depth
        ihdr.push(6); // color type: RGBA
        ihdr.push(0); // compression
        ihdr.push(0); // filter
        ihdr.push(0); // interlace

        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']);
        png.extend_from_slice(&chunk(b"IHDR", &ihdr));
        png.extend_from_slice(&chunk(b"IDAT", &[]));
        png.extend_from_slice(&chunk(b"IEND", &[]));

        let path = tmp_path("huge-header.png");
        std::fs::write(&path, &png).unwrap();
        assert!(
            wal_colors(&path).is_err(),
            "a 65500x65500 16-bit RGBA header must be rejected without allocating"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Confirms the budget decision stated on [`crate::jpeg::MAX_DECODE_BUDGET_BYTES`]:
    /// a real, fully realized 16-bit RGBA wallpaper at 16K resolution
    /// (15360x8640, ~1 GiB decoded) must still succeed, not just get
    /// rejected like the crafted header above. Heavy (allocates and
    /// encodes over a gigabyte); not run by default.
    #[test]
    #[ignore = "allocates and encodes a ~1 GiB image; run explicitly"]
    fn a_real_16k_16bit_rgba_wallpaper_still_decodes() {
        let (w, h) = (15360u32, 8640u32);
        let mut img = image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::new(w, h);
        for (x, y, px) in img.enumerate_pixels_mut() {
            let v = ((x + y) % 65536) as u16;
            *px = image::Rgba([v, v.wrapping_add(1), v.wrapping_add(2), u16::MAX]);
        }
        let path = tmp_path("real-16k-16bit.png");
        image::DynamicImage::ImageRgba16(img).save(&path).unwrap();
        let result = wal_colors(&path);
        let _ = std::fs::remove_file(&path);
        assert!(
            result.is_ok(),
            "a real 16K 16-bit RGBA wallpaper must decode within the budget: {result:?}"
        );
    }
}
