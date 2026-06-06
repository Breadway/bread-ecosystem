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
| `bread` | Reactive automation daemon (`breadd`) + CLI |
| `breadbar` | Status bar and notification daemon |
| `breadbox` | Cloud sync daemon (`breadbox-sync`) + file browser |
| `breadcrumbs` | Network information CLI |
| `breadpad` | Scratchpad / quick-note app |

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

| Package | Arch packages |
|---------|--------------|
| `bread` | `libudev` `dbus` |
| `breadbar` | `gtk4` `gtk4-layer-shell` `dbus` `iw` |
| `breadbox` | `gtk4` `librsvg` `dbus` |
| `breadcrumbs` | `networkmanager` |
| `breadpad` | `gtk4` `dbus` |

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

## License

MIT
