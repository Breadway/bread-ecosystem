# Bread Ecosystem

A collection of Rust tools for the Linux desktop (Hyprland / Wayland / Arch).
Install any product with a single command — no Rust toolchain required.

```sh
curl -fsSL https://get.breadway.dev | sh
bakery install breadbar
```

## Products

The table below is generated from [`registry/bread-ecosystem.toml`](registry/bread-ecosystem.toml). Regenerate with `scripts/gen-readme-products.sh`.

<!-- gen-readme-products:start -->

| Package | Description |
|---------|-------------|
| `bakery` | Bread ecosystem package manager |
| `bread-theme` | Shared pywal-accented, fixed-dark-base theming CLI for the bread ecosystem |
| `bread` | Reactive automation daemon and CLI for Linux desktops |
| `breadbar` | Minimal status bar and notification daemon for Hyprland |
| `breadbox` | App launcher for Hyprland / Wayland |
| `breadcrumbs` | Profile-aware Wi-Fi state machine with Tailscale integration |
| `breadpad` | Quick-capture scratchpad and note viewer with AI classification |
| `breadpaper` | Wallpaper manager for the bread desktop |
| `breadmon` | Terminal UI monitor manager for Hyprland |
| `breadsearch` | Semantic system-wide search for BOS |
| `breadclip` | Wayland clipboard history manager for Hyprland |
| `breadshot` | Screenshot utility for the bread ecosystem |
| `bos-settings` | System settings app for Bread OS |
| `breadhelp` | Onboarding and help center for Bread OS |
| `breadcast` | Cast your screen to any Chromecast/Google TV or DLNA renderer — daemon + GTK4 popup — Bakery product; not included in the BOS ISO |
| `breadarr` | Single-daemon Sonarr+Radarr+Prowlarr replacement — release watching, matching, grabbing, importing, and a terminal UI, no web UI — Homelab, not shipped on BOS |

<!-- gen-readme-products:end -->

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

All GUI products (breadbar, breadbox, breadpad) share one stylesheet via
`bread-theme`. Background, surface, overlay, and foreground are always BOS's
fixed dark values; only the accent colors are read from the pywal palette in
`~/.cache/wal/colors.json`. When that file is absent, the accents fall back
to BOS's curated bread-toned defaults (not Catppuccin Mocha). The stylesheet
is written to `$XDG_RUNTIME_DIR/bread/theme.css`; running apps watch that
file and recolour live when it changes. Per-app CSS overrides live at
`~/.config/<app>/style.css`.

```sh
wal -i ~/Pictures/wall.png   # regenerate pywal palette
bread-theme generate         # render the shared stylesheet (run from a wal hook)
```

`bread-theme` subcommands:

| Subcommand | Description |
|------------|-------------|
| `generate` | Render the current palette and write the shared stylesheet (default) |
| `reload` | Same as `generate`; use after a palette change to trigger live recolour in running apps |
| `path` | Print the stylesheet path |
| `print` | Render the stylesheet to stdout without writing |
| `layerrules` | Write the active shell theme's `[compositor]` rules to `~/.config/hypr/layerrules.json` |
| `describe` | Print the `theme.toml` schema as JSON |
| `diagnose <id>` | Exit 0 if theme `<id>` resolves, else print why and exit 1 |

The shared theming logic lives in the `bread-theme` crate in this repo. See
[`BREAD_DESIGN_SYSTEM.md`](BREAD_DESIGN_SYSTEM.md) for the design tokens (fonts,
spacing, radii, colour roles) the stylesheet is built from.

### Shell themes

Beyond the shared colour stylesheet, a **shell theme** (`theme.toml`) describes
the whole shell's shape and structure — the bar's geometry and contents, the
launcher, the notification/OSD/panel surfaces, and their compositor rules. The
active one is selected in `~/.config/bread/shell.toml` (`active = "<id>"`);
five are built in (`liquid-motion`, `glass-workbench`, `spotlight`, `daylight`,
`loaf`) and user themes live in `~/.config/bread/themes/<id>/`. See
[`docs/shell-themes.md`](docs/shell-themes.md) for the authoring guide.

## Installing bakery

`bakery` is the package manager for the ecosystem. Install it with the bootstrap script:

```sh
curl -fsSL https://get.breadway.dev | sh
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

## System prefix (BOS)

Default install root is `~/.local`. BOS sets a system prefix so bakery-managed
desktop apps live on the `@` root subvolume and ride along with
snapper/grub-btrfs snapshots:

```toml
# /etc/bakery/config.toml
prefix = "/usr/local"
```

`BAKERY_PREFIX` overrides the config file. A non-home prefix installs bins to
`$prefix/bin`, share/data/desktop/licenses to `$prefix/share/...`, and systemd
user units to `/usr/lib/systemd/user`. Per-user state (`installed.json`,
update backups) stays in `~/.local/state/bakery`. Writes that need root use
`sudo -n`, then `pkexec`. `bakery doctor` prints the active prefix.

Hermes and `get.sh` are unchanged — they keep the user-local default. See
[`bakery/README.md`](bakery/README.md).

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

## Workspace

This repo is a Cargo workspace. Bakery-channel products shipped from here
are `bakery` and `bread-theme`; the other members are shared crates sibling
apps pin, or in-tree tools that are not bakery packages of their own.

```
bread-ecosystem/
├── bakery/              # package manager binary
├── bread-theme/         # shared pywal + fixed-dark-base theming crate
├── bread-utils/         # shared plumbing (Hyprland IPC, singleton, XDG, BreadClient, …)
├── bread-app/           # GTK bootstrap new tools should use (app id, singleton, overlay, command listen)
├── bread-polkit/        # themed PolicyKit agent (bakery.toml present; unpublished)
├── bread-onnx/          # shared ONNX runtime helpers
├── bread-screenshots/   # grim capture primitive used by app `--screenshot` modes
├── bread-capture/       # orchestrator that drives those `--screenshot` modes
├── registry/            # bread-ecosystem.toml — product registry
└── scripts/
    ├── get.sh                   # curl | sh bootstrap
    ├── gen-index.sh             # generates dl.breadway.dev/index.json from release artifacts
    └── gen-readme-products.sh   # rewrites the Products table from the registry
```

### New GTK tools

Do not copy another app's `main.rs`. Depend on `bread-app`:

- `bread_app::application_id` / `try_acquire` / `toggle_or_kill` for the
  `com.breadway.*` application id and single-instance lock
- feature `gtk` re-exports `bread_utils::gtk_popup` (layer-shell overlay)
- feature `bread-client` for `listen_commands` on `bread.command.<app>.**`

See the `bread-app` crate docs. Existing apps are not migrated in this
tree; `bread-polkit` is the first in-tree consumer.

### bread-polkit

A session PolicyKit authentication agent (password prompt, cancel,
identity). Not a wrapper around `polkit-gnome`. `bread-polkit/bakery.toml`
exists so it can be published via bakery; it is not in
`registry/bread-ecosystem.toml` and is therefore unpublished — not on the
bakery index and not on the BOS ISO lockfile.

```sh
cargo run -p bread-polkit
```

Autostart — pick one:

```sh
cp bread-polkit/contrib/bread-polkit.desktop ~/.config/autostart/
```

```
# hyprland.conf
exec-once = bread-polkit
```

## Release pipeline

Each product repo (`Breadway/bread`, `Breadway/breadbar`, …) has
`.forgejo/workflows/release-*.yml` that triggers on `v*` tags. The workflow
runs on a self-hosted runner on hestia, builds a stripped x86_64 binary,
deposits it at `dl.breadway.dev/<pkg>/<version>/`, updates `index.json`,
and mirrors the binary to GitHub Releases as a fallback.

`bakery` always tries `dl.breadway.dev` first and transparently falls back
to the GitHub Release URL recorded in the manifest.

Beyond stable releases, most products also publish **dev** and **beta**
tracks — continuous builds off `main` (dev) and `vX.Y.Z-rc.N` tags (beta).
See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the branch/release workflow and
[`docs/release-channels.md`](docs/release-channels.md) for the full track
policy. Switch tracks with `bakery track set <stable|beta|dev>`.

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
