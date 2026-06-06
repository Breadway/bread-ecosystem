# bread-theme changelog

## Coordinated bump policy

`bread-theme` is consumed by `breadbar`, `breadbox`, and `breadpad` as a pinned
git dependency. A breaking change to `Palette`, `css_vars`, or the `gtk` feature
API requires all three dependents to bump their `Cargo.toml` git tag and cut a
release together. Note the impact in this file before tagging.

---

## theme-v0.1.0 (2026-06-06)

- Initial extraction from `breadpad-shared/src/theme.rs`
- `Palette` struct with `color0`–`color7` and Catppuccin Mocha default
- `load_palette()` reads `~/.cache/wal/colors.json`, falls back to default
- `css_vars(palette)` emits `@define-color` block + font declaration
- `hex_to_rgba(hex, alpha)` utility
- `tokens` module with spacing scale, border radii, font sizes from `BREAD_DESIGN_SYSTEM.md`
- `gtk` feature: `apply_css()` and `apply_user_css()` helpers for GTK4 CSS providers
