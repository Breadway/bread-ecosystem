# Bread Ecosystem

A collection of Rust tools for the Linux desktop (Hyprland / Wayland / Arch).
Install any product with a single command — no Rust toolchain required.

```sh
curl https://breadway.dev/get | sh
bakery install breadbar
```

## Products

| Package | Description |
|---------|-------------|
| `bread` | Reactive automation daemon (`breadd`) + CLI — Lua scripting over Hyprland, udev, power, network, and Bluetooth events |
| `breadbar` | GTK4 status bar (workspaces, clock, CPU/RAM/battery/WiFi/Bluetooth) and D-Bus notification daemon for Hyprland |
| `breadbox` | GTK4 fuzzy app launcher for Hyprland with context-aware sorting; ships an icon-sync daemon (`breadbox-sync`) |
| `breadcrumbs` | Profile-aware Wi-Fi state machine with Tailscale exit-node management and a self-healing watch daemon |
| `breadpad` | Quick-capture scratchpad popup with AI-powered note classification, reminders, recurrence, and a full note viewer (`breadman`) |

## Recommended keybinds

The ecosystem assumes a Hyprland setup with `SUPER` as the modifier. The
conventional bindings (used by BOS and recommended for any install):

| Keys | Action |
|------|--------|
| `SUPER+Space` | `breadbox` — app launcher |
| `SUPER+U` | `breadpad` — quick-capture notes/reminders |
| `SUPER+M` | `breadman` — note viewer / manager |
| `SUPER+,` | settings (`bos-settings`, where installed) |

`breadbar` and `breadd` are services started at login (`exec-once`), not bound
to keys.

## Theming

All GUIs share one look via `bread-theme`. The `bread-theme` CLI renders the
component stylesheet from your pywal palette (Catppuccin Mocha fallback) to
`$XDG_RUNTIME_DIR/bread/theme.css`; every app loads that file and **live-reloads**
it, so changing your wallpaper recolours the whole ecosystem with no rebuilds:

```sh
wal -i ~/Pictures/wall.png   # regenerate pywal palette
bread-theme generate         # render the shared stylesheet (run from a wal hook)
```

See [`BREAD_DESIGN_SYSTEM.md`](BREAD_DESIGN_SYSTEM.md) for the tokens (fonts,
spacing, radii, colour roles) the stylesheet is built from.

## Installing bakery

`bakery` is the package manager for the ecosystem. Install it with the bootstrap script:

```sh
curl https://breadway.dev/get | sh
# or
curl -sSfL https://get.breadway.dev | sh
```

The script downloads the prebuilt `bakery` binary to `~/.local/bin/bakery` and prints a note if that directory isn't on your `PATH` yet.

## Using bakery

```sh
bakery list                    # all available packages
bakery list --installed        # only installed packages
bakery info breadbar           # version, binaries, system deps, services
bakery doctor                  # check system deps for installed packages
bakery doctor breadbar         # check system deps for a specific package

bakery install <pkg>           # install a package
bakery update <pkg>            # update a package
bakery update --all            # update everything
bakery remove <pkg>            # remove a package (data files are never deleted)
```

`bakery install` runs `doctor` first and bails with a clear message if any system dependency is missing. Binaries land in `~/.local/bin` (override with `BAKERY_BIN_DIR`).

## System dependencies by product

`bakery doctor` checks these automatically before any install. Required deps block installation; optional deps generate a warning but never block.

| Package | Required | Optional |
|---------|----------|---------|
| `bakery` | _(statically linked, none)_ | — |
| `bread` | `systemd-libs` `openssl` `zlib` | `bluez` `hyprland` |
| `breadbar` | `gtk4` `gtk4-layer-shell` `iw` `libpulse` | `hyprland` |
| `breadbox` | `gtk4` `gtk4-layer-shell` `librsvg` | `hyprland` |
| `breadcrumbs` | `networkmanager` | `tailscale` `sudo` `xdg-utils` |
| `breadpad` | `gtk4` `gtk4-layer-shell` | `rocm-hip-runtime` `ollama` `hyprland` |

Install all required deps with `sudo pacman -S <packages>`. Use `pacman -Q <pkg>` to check whether any are already present.

## Theming

All GUI products (breadbar, breadbox, breadpad) read pywal colors from
`~/.cache/wal/colors.json` and fall back to Catppuccin Mocha when that file
is absent. Per-app CSS overrides live at `~/.config/<app>/style.css`.

The shared theming logic lives in the `bread-theme` crate in this repo.

## Workspace

This repo is a Cargo workspace:

```
bread-ecosystem/
├── bakery/          # package manager binary
├── bread-theme/     # shared pywal + Catppuccin theming crate
├── registry/        # bread-ecosystem.toml — product registry
└── scripts/
    ├── get.sh       # curl | sh bootstrap
    └── gen-index.sh # generates dl.breadway.dev/index.json from release artifacts
```

## Release pipeline

Each product repo (`Breadway/bread`, `Breadway/breadbar`, …) has a
`.github/workflows/release.yml` that triggers on `v*` tags. The workflow
runs on a self-hosted runner on hestia, builds a stripped x86_64 binary,
deposits it at `dl.breadway.dev/<pkg>/<version>/`, updates `index.json`,
and mirrors the binary to GitHub Releases as a fallback.

`bakery` always tries `dl.breadway.dev` first and transparently falls back
to the GitHub Release URL recorded in the manifest.

### Release artifact contract

Each product's `release.yml` **must** upload the following files alongside
the binary to `dl.breadway.dev/<name>/<version>/`:

| File | Purpose |
|------|---------|
| `bakery.toml` | Metadata (deps, services, config) read by `gen-index.sh` |
| `<binary>-x86_64.sha256` | Checksum verified by `bakery install` and `get.sh` |
| `*.service` | systemd unit files installed by `bakery install` |
| `*.example.toml` / `config.example.toml` | Example configs copied on first install |

`gen-index.sh` **fails loudly** if `bakery.toml` is missing — this is by
design to catch omissions in the release workflow before they silently
produce empty metadata in production.

## License

MIT
