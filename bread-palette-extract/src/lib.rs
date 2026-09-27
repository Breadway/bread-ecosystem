//! Native (no `wal`/ImageMagick subprocess) port of the palette-extraction
//! pipeline `wal -i <image>` runs: a faithful Rust translation of the
//! relevant ImageMagick 7.1.2 (Q16 HDRI) code paths (resize, quantize,
//! dither, unique-colors) and, for JPEGs, of libjpeg-turbo's default decode.
//!
//! Split out of `bread-theme` into its own crate (ECO-30/ECO-33) purely for
//! licensing: this crate's own code is a derivative work of ImageMagick and
//! libjpeg-turbo/IJG, so it carries their licence terms (see `NOTICE`,
//! `LICENSE-ImageMagick`, `LICENSE-libjpeg-turbo`), while `bread-theme`
//! itself stays plain MIT and only depends on this crate behind its
//! optional `extract` feature.
//!
//! [`wal_colors`] is the entire public surface: the full ordered colour
//! list pywal's default backend builds for an image. Slicing out
//! `color1..color6` and choosing a fallback for images wal itself would
//! reject are bread-specific policy, not part of what wal/ImageMagick
//! themselves do, so they live in `bread-theme` instead of here.

mod extract;
mod jpeg;

pub use extract::wal_colors;

/// This crate's own NOTICE text: what it ports from ImageMagick and
/// libjpeg-turbo/IJG, and where. See `NOTICE` at this crate's root.
pub const NOTICE: &str = include_str!("../NOTICE");
/// The ImageMagick License, reproduced verbatim. See `LICENSE-ImageMagick`
/// at this crate's root.
pub const LICENSE_IMAGEMAGICK: &str = include_str!("../LICENSE-ImageMagick");
/// The libjpeg-turbo/IJG licence terms, reproduced verbatim. See
/// `LICENSE-libjpeg-turbo` at this crate's root.
pub const LICENSE_LIBJPEG_TURBO: &str = include_str!("../LICENSE-libjpeg-turbo");
