# bread-theme changelog

## 0.7.8

Expanded shell-theme UI control (four axes):

- **Typography** — `font_family` / `font_fallback` / `font_size_base` and a
  new `font_weight` token now drive the whole bar + launcher **and** the
  shared ecosystem stylesheet (`stylesheet()` takes a `Typography` resolved
  from `shell::load()`), not just breadbox's launcher panel. `font_family_css`
  builds a well-formed quoted family list. `liquid-motion` pinned to the
  historical `tokens::FONT_*` values so a default install is unchanged;
  glass-workbench / spotlight / daylight render their intended fonts.
- **Chrome tokens** — `sep_style`, `bar_shadow`, `chip_gap`,
  `ws_gradient_angle`, `media_eq_style`, `osd_style`, all defaulting to the
  current rendering.
- **`[panel]` / `[osd]`** — `[panel]` `min_width` + `sections` (reorder / drop
  the control-panel body sections); `[osd]` `enabled` + `dismiss_ms`.
- **`[[bar.widget]]`** — a theme can declare a live bar widget inline: a
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
fixed — only color1–6 come from the wallpaper.

On disk under `$XDG_RUNTIME_DIR/bread/` (same fallback as `shared_css_path`):

- `palettes/<sanitized-output>.json` — accents only (round-trips through
  `from_wal_json` / a color1–6 object; never persists pywal's light bg)
- `themes/<sanitized-output>.css` — `stylesheet()` for that palette

New lib API:

- `themes_dir`, `palettes_dir`, `output_css_path`, `output_palette_path`,
  `sanitize_output`
- `load_palette_for`, `write_output_palette`, `write_output_css`,
  `write_shared_css_from`
- `palette_from_image` (isolated `wal -i`, does not touch `~/.cache/wal`),
  `generate_output`, `palette_from_json`
- `stylesheet_resolved` — inlines `@accent` / `@on-bg` / … to hex so GTK's
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
