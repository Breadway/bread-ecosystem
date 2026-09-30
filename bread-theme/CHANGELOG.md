# bread-theme changelog

## Unreleased

- ECO-35: stop reading `~/.cache/wal/colors.json` as a palette fallback and
  remove the unused `wal_cache_path` and `write_wal_cache` APIs. Keep wal-shaped
  JSON imports and `wal_cache_json` for the bread cache and third-party templates.
  Merge and release this removal only after breadpaper and BOS have shipped the
  bread-owned cache path for at least one full release cycle.

## 0.8.0

Epic PYWAL (ECO-29..33): removes the `wal`/ImageMagick subprocess dependency
entirely.

- ECO-30: `palette_from_image` is now a native Rust extractor
  (`bread-palette-extract/src/extract.rs`) that reproduces `wal -i` exactly:
  a port of the ImageMagick 7.1.2 (Q16 HDRI) code behind pywal's
  `magick "<img>[0]" -resize 25% -colors N -unique-colors txt:-` (resize
  filter selection and two-pass resampling, octree classify/reduce/colormap,
  the default Riemersma dither and its colour cache, `-unique-colors`
  ordering, `txt:` hex formatting including pywal's `#.{6}` truncation of
  16-bit output), plus a JPEG decoder ported from libjpeg-turbo so JPEG
  samples match what ImageMagick decodes. Measured against real pywal:
  every slot of the full colour list matches on all committed fixtures, and
  all accents match on 21 real wallpapers plus the BOS default
  (`bread-palette-extract/scripts/compare-pywal.py`). Decodes PNG, JPEG,
  WebP, GIF (first frame) and BMP. New `pywal_colors()` returns the full
  list. Third-party notices: `NOTICE`, `bread-theme notice`. No more
  subprocess spawn, no more isolated `HOME`/`XDG_CACHE_HOME` temp-dir
  dance, no more "wal is not installed" error path.
- ECO-29: golden pywal output for reference images
  (`bread-palette-extract/tests/fixtures/`, captured by
  `bread-palette-extract/scripts/capture-goldens.py` from real `wal -i` in
  an isolated `HOME` plus pywal's own `gen_colors`) feeds exact per-slot and
  full-list tests in `bread-palette-extract/tests/golden.rs`, plus
  gated tests in `bread-theme/src/output.rs` that exercise the real
  `palette_from_image` public API (slicing, the default-accent fallback,
  and a decode error) against the same fixtures. Fixtures cover 8-bit,
  palette, gray and 16-bit PNG, baseline and progressive JPEG, WebP, BMP
  and an animated GIF.
- ECO-31: the session-global palette moves to a bread-owned cache,
  `palette_cache_path()` (`$XDG_CACHE_HOME/bread/palette.json`, wal
  `colors.json` shape), written atomically by the new `write_global_palette`.
  `load_palette()` reads that path first, then falls back to
  `~/.cache/wal/colors.json` for installs upgrading from a `wal`-based
  setup, then `Palette::default()`. `palette_cache_dir()` and
  `sequences_path()` are also exported: the former is a stable directory a
  long-lived watcher (breadman) can watch, the latter is where the next
  point writes.
- ECO-32: `write_global_palette` also writes a terminal colour-sequences
  file (`sequences_path()`, OSC 4 for all 16 ANSI slots plus OSC 10/11/12
  for fg/bg/cursor) alongside the palette cache, so BOS's skel `.zshrc`
  (which cats this file to colour new terminal sessions) keeps working now
  that nothing runs `wal` to produce it. Terminals also now get the fixed
  BOS dark background instead of a raw wallpaper-derived one.
- ECO-33: `python-pywal` dropped from `bakery.toml`'s optional system deps;
  Cargo.toml/bakery descriptions, CLI help, README, BREAD_DESIGN_SYSTEM.md,
  and docs/shell-themes.md reworded to describe native extraction instead
  of pywal. The bread-ecosystem release that ships this needs to go out
  before any consumer (breadpaper, etc.) is updated to depend on it, since
  roughly a dozen apps pin bread-theme by tag.
- Licensing (user decision): the extractor (`extract.rs`, `jpeg.rs`, their
  fixtures/tests/scripts, `NOTICE`, `LICENSE-ImageMagick`,
  `LICENSE-libjpeg-turbo`) moved out of `bread-theme` into a new workspace
  crate, `bread-palette-extract`, licensed `ImageMagick AND IJG` (the ported
  libjpeg-turbo files belong to its core libjpeg API library, which is
  IJG-only; the separate Modified 3-clause BSD terms cover the TurboJPEG
  API/cjpeg/djpeg/build system, none of which is used here, so it's dropped
  from the SPDX expression). `bread-theme` itself is plain `MIT` again and
  depends on `bread-palette-extract` only behind a new, off-by-default
  `extract` feature: `palette_from_image`, `pywal_colors`,
  `generate_output`, and `generate-output --image`/`notice`'s third-party
  section are the only things gated by it (a build without it gets a clear
  error on `--image` instead of a missing binary or a silent wrong result).
  Everything else (`Palette`, `load_palette`, `palette_cache_*`,
  `write_global_palette`, sequences, per-output writers, `palette_from_json`)
  is unaffected. The bakery build of the `bread-theme` binary enables
  `extract` (there is no separate pacman build of bread-theme itself); CI
  now runs fmt/clippy/test/build with and without it.
  `NOTICE` also now reproduces the one entry of ImageMagick's own upstream
  NOTICE file that pertains to the ported code, as the ImageMagick License's
  redistribution clause requires.
- ECO-30 hardening (second review round): the JPEG port sized its
  coefficient/plane/output buffers straight from the untrusted SOF header,
  so a 22 KB file declaring 65500x65500 drove RSS to 11.4 GB before OOM
  killed it. All three allocations now check against a single documented
  2 GiB budget (`MAX_DECODE_BUDGET_BYTES`) before allocating anything,
  comfortably covering a real 16K (15360x8640) 16-bit RGBA wallpaper with
  headroom; the `image`-crate path (png/webp/gif/bmp) now uses the same
  figure as an explicit `image::Limits` instead of that crate's undocumented
  512 MiB default (which was in fact too small for a real 12000x8000 16-bit
  RGBA PNG). A JPEG whose entropy-coded data runs out before a scan
  finishes (the file ends with no marker to recover at) now fails outright
  (`UnexpectedEof`) instead of silently returning wrong-but-plausible
  colours from the ported decoder's zero-padding past the cut. A scan cut
  short at a marker boundary still zero-fills the rest of that scan, same
  as libjpeg-turbo/ImageMagick's own recovery (real `wal` also exits 0 on
  such a file), so that case is unaffected.
- ECO-30 hardening (third review round): a JPEG whose SOF header alone
  exceeds `MAX_DECODE_BUDGET_BYTES` now fails outright
  (`io::ErrorKind::InvalidData`) instead of falling back to the `image`
  crate, which was bypassing the budget entirely and decoding the same
  oversized claim with its own, unbounded allocator (a 20000x20000 lie on a
  22 KB real fixture reached ~1.9 GB RSS and 21 s of CPU before this fix).
  The budget sum also now counts the JPEG port's per-component upsample
  output buffers, not just the coefficient/plane/RGB buffers, closing a gap
  where real peak usage could run about `3 * width * height` bytes past the
  documented budget. `bread-theme/src/output.rs` gained its own
  `--features extract` tests that call the real `palette_from_image` public
  API (not just `bread-palette-extract`'s internal slice) against the
  golden fixtures, the default-accent fallback path, and a decode error.

## 0.7.11

Adds ECO-22: `write_wal_cache(&Palette, path)`, plus `wal_cache_json` and
`wal_cache_path`. bread-theme could already read pywal's `colors.json`
(`load_palette`) but had no way to write that format from an in-memory
`Palette`. The write is atomic (temp file then rename), reusing the same
path the rest of bread-theme's file writes use.

Motivating consumer: breadpaper's `sync_focused_global` holds a per-output
`Palette` but still shells out to a second `wal -i` on every focused
`set_on` just to refresh `~/.cache/wal/colors.json`: a process spawn per
switch, and it re-extracts from the image rather than using the palette
already in hand, so the cache and breadpaper's own per-output palette can
drift. It can now write the cache directly (that change ships in the
breadpaper repo). All 16 ANSI slots and `special` are written so the cache
stays valid for other pywal consumers; `color8..color15` mirror
`color0..color7`.

---

Fixes ECO-17 (text clipping in breadbar on custom themes): `[tokens]
chip_height` was parsed, stored, and even documented, but never actually
reached the CSS it's supposed to control: `chip_height_px` (the
`min-height` baked into `.media-widget`, `.stat-pair`, and Trail/Pill
`.workspace-btn`) came from a hardcoded per-`WorkspaceStyle` table instead,
completely independent of the token. Every builtin theme happened to set
`chip_height` to the same number that table already returns for its style
(that's how this went unnoticed), except `loaf`, which explicitly asked for
`24` and silently got `22` (inherited via `extends = "glass-workbench"`),
proof the token was dead even for a shipped theme, not just a hypothetical.

A theme with a custom `font_family`/`font_size_base`/`font_weight` sized for
a taller chip than its style's demo number had no way to ask for more room,
so descenders on `.media-widget`/chip labels clipped against the box.

`resolve_tokens` now falls back to the per-style number only when a theme
doesn't set `chip_height` at all; an explicit value is used as-is. `loaf`'s
golden CSS snapshot updates from 22px to 24px accordingly; every other
builtin is byte-identical.

## 0.7.10

Vertical (side-docked) bars: `[bar.window].anchors` anchored to exactly one
of `left`/`right` plus both `top` and `bottom` now resolves as a vertical
dock instead of the usual horizontal top/bottom bar: no new schema field,
derived from the anchor set itself, so every existing theme is unaffected.
`width` becomes the dock's fixed thickness in that case (`"fill"` is now a
hard error there, the fill axis for a side dock is top/bottom, already
filled by the anchors). New `WindowSpec::orientation()` /
`WindowOrientation`. See `docs/shell-themes.md`'s `[bar.window]` section.
Consumer-side layout (breadbar) is a separate release.

## 0.7.9

`window.breadbar-panel` (the control-panel + connectivity popover) now takes
its corner radius from `radius_card`, like every other popover card
(`window.breadbar-history`, `.notification-card`, `window.wifi-add-dialog`),
it was the one surface still hardcoded to `14px`, so a hard-edged theme's
panel came out rounded. Default (`liquid-motion`) 14→12px.

## 0.7.8

Expanded shell-theme UI control (four axes):

- **Typography**: `font_family` / `font_fallback` / `font_size_base` and a
  new `font_weight` token now drive the whole bar + launcher **and** the
  shared ecosystem stylesheet (`stylesheet()` takes a `Typography` resolved
  from `shell::load()`), not just breadbox's launcher panel. `font_family_css`
  builds a well-formed quoted family list. `liquid-motion` pinned to the
  historical `tokens::FONT_*` values so a default install is unchanged;
  glass-workbench / spotlight / daylight render their intended fonts.
- **Chrome tokens**: `sep_style`, `bar_shadow`, `chip_gap`,
  `ws_gradient_angle`, `media_eq_style`, `osd_style`, all defaulting to the
  current rendering.
- **`[panel]` / `[osd]`**: `[panel]` `min_width` + `sections` (reorder / drop
  the control-panel body sections); `[osd]` `enabled` + `dismiss_ms`.
- **`[[bar.widget]]`**: a theme can declare a live bar widget inline: a
  `bind = { cmd, every }` poll + a box/label/icon/progress node tree with
  `{value}` substitution, rendered through breadbar's existing `WidgetSpec`
  path. Node depth/count bounded (4/50); style vocab validated.
- A standalone theme (neither a builtin nor extending one) now renders from
  the shared `base.css` instead of an empty template. `.popover-caret`
  gradient de-hardcoded to `accent_from`/`accent_to`. `bread-theme describe`
  gains `panel` / `osd` / `bar.widget` blocks.

## 0.7.4

Per-output (per-monitor) theming. Session-global `theme.css` remains the
fallback / focused-monitor sheet; each Hyprland/GDK connector can now have
its own palette and stylesheet. BOS still keeps bg/surface/overlay/fg
fixed: only color1–6 come from the wallpaper.

On disk under `$XDG_RUNTIME_DIR/bread/` (same fallback as `shared_css_path`):

- `palettes/<sanitized-output>.json`: accents only (round-trips through
  `from_wal_json` / a color1–6 object; never persists pywal's light bg)
- `themes/<sanitized-output>.css`: `stylesheet()` for that palette

New lib API:

- `themes_dir`, `palettes_dir`, `output_css_path`, `output_palette_path`,
  `sanitize_output`
- `load_palette_for`, `write_output_palette`, `write_output_css`,
  `write_shared_css_from`
- `palette_from_image` (isolated `wal -i`, does not touch `~/.cache/wal`),
  `generate_output`, `palette_from_json`
- `stylesheet_resolved`: inlines `@accent` / `@on-bg` / … to hex so GTK's
  display-global `@define-color` cannot leak the wrong monitor's accent

GTK (`gtk` feature): `bind_window`, `bind_window_with_app_css`,
`output_for_widget`, `bind_window_auto`, `bind_window_auto_with_app_css`.
Widget-scoped providers at `USER - 10` so they beat `apply_shared` but
lose to user CSS. Existing `apply_shared` / `apply_app_css` /
`apply_css` / `apply_user_css` are unchanged.

CLI: `bread-theme generate-output <OUTPUT> --image <PATH> | --from-json
<FILE> [--shared]`. Does not write `theme.css` unless `--shared`.

## Coordinated bump policy

`bread-theme` is consumed by `breadbar`, `breadbox`, `breadpad`, and the other
GTK bread apps as a pinned git dependency. A breaking change to `Palette`,
`css_vars`, or the `gtk` feature API requires dependents to bump their
`Cargo.toml` git tag and cut a release together. Note the impact in this file
before tagging.

**0.7.4** adds per-output bind APIs (`bind_window*`, `load_palette_for`,
`generate_output`). Apps that call those must pin `tag = "v0.7.4"`.

---

## theme-v0.1.0 (2026-06-06)

- Initial extraction from `breadpad-shared/src/theme.rs`
- `Palette` struct with `color0`–`color7` and Catppuccin Mocha default
- `load_palette()` reads `~/.cache/wal/colors.json`, falls back to default
- `css_vars(palette)` emits `@define-color` block + font declaration
- `hex_to_rgba(hex, alpha)` utility
- `tokens` module with spacing scale, border radii, font sizes from `BREAD_DESIGN_SYSTEM.md`
- `gtk` feature: `apply_css()` and `apply_user_css()` helpers for GTK4 CSS providers
