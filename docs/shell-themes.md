# Authoring a BOS shell theme

A **shell theme** is a single `theme.toml` file that describes the whole BOS
shell — the bar's shape and contents (`breadbar`), the app launcher
(`breadbox`), the notification / OSD / control-panel surfaces, and the
compositor rules that go with them. As of `bread-ecosystem` **v0.7.8** the
manifest drives rendering end to end: geometry, structure, style, typography,
the control-panel layout, the OSD set, and even a live bar widget all come
from the document. There are no per-theme CSS files and no code to write for a
new look.

This guide covers writing one. For the machine-readable schema, run
`bread-theme describe`.

---

## Where the pieces live

A theme is a directory named for its id, containing `theme.toml` and
optionally `extra.css`:

```
<id>/
├── theme.toml      # required — the manifest
└── extra.css       # optional — a raw-CSS overlay (see "extra.css" below)
```

A theme id resolves through these locations, **first hit wins**:

| Priority | Path |
|---|---|
| 1 | `~/.config/bread/themes/<id>/theme.toml` (user) |
| 2 | `/usr/share/bread/themes/<id>/theme.toml` (system / BOS package) |
| 3 | a compiled-in builtin |

The five builtins — `liquid-motion` (default), `glass-workbench`, `spotlight`,
`daylight`, `loaf` — have no on-disk directory; their `theme.toml` text lives
in `bread-theme/assets/shell/<id>/theme.toml` and is the best set of worked
examples to read.

### Choosing the active theme

`~/.config/bread/themes/…` and the builtins are just *available* themes. The
**active** one is set in a separate file, `~/.config/bread/shell.toml`:

```toml
active = "loaf"
```

Every bread app reads this same file, so it is the only mechanism that keeps
two already-running processes (breadbar and breadbox) on the same theme. Edit
it and save — the running shell picks up the change (see
[Live switch vs. restart](#live-switch-vs-restart)).

`$BREAD_SHELL_THEME` overrides `active` for **one process only**, read once at
launch. It is a development convenience for `breadbar --screenshot` and the
like — not a way to switch a running system.

---

## Quick start

The builtins are compiled into `bread-theme` — they have no on-disk copy to
`cp`. The two starting points are: **extend a builtin** and override only what
you're changing (best for a tweak), or **copy a builtin's source** from
`bread-ecosystem/bread-theme/assets/shell/<id>/theme.toml` (best for a
ground-up look).

```sh
# 1. Create the theme directory (named for the id).
mkdir -p ~/.config/bread/themes/mytheme

# 2. Write theme.toml. A minimal theme that just tweaks a builtin:
cat > ~/.config/bread/themes/mytheme/theme.toml <<'EOF'
name    = "My Theme"
id      = "mytheme"
extends = "glass-workbench"

[tokens]
radius_bar   = 18
radius_card  = 16
bg_alpha     = 0.85
accent_from  = "blue"
accent_to    = "blue"

[bar.window]
anchors = ["top", "left", "right"]
width   = "fill"
height  = 38
margin  = { top = 10, left = 16, right = 16 }
EOF

# 3. Check it resolves. Prints nothing and exits 0 on success; on failure
#    prints the reason and exits 1 (instead of the shell silently falling
#    back to liquid-motion).
bread-theme diagnose mytheme

# 4. Activate it.
sed -i 's/^active = .*/active = "mytheme"/' ~/.config/bread/shell.toml
```

If step 3 fails, the message names the offending key — a typo'd token, an
unknown enum value, or a `{ref}` in your CSS that matched nothing.

A directory under `themes/` with no `theme.toml` in it is ignored, so empty
`glass-workbench/`, `loaf/` etc. dirs (other tools sometimes create them) are
harmless — discovery falls straight through to the compiled-in builtin.

---

## Anatomy of a `theme.toml`

### Identity

```toml
name = "My Theme"      # display name (bos-settings, pickers)
id   = "mytheme"        # must match the directory name
```

### `extends` — inherit from another theme

```toml
extends = "glass-workbench"
```

One level deep, deep-merged: your file is layered over the base's, key by key,
recursing into tables. **Scalars and whole tables** merge (child wins);
**arrays replace wholesale** unless you use the splice marker (below). A chain
longer than one level is not followed — if the base itself has `extends`, that
is dropped before merging.

#### The `"+"` slot splice

Inside a `[bar.slots]` list, `"+"` is replaced in place by the base theme's
list for that same slot, so a child can add one entry without restating the
parent's:

```toml
# parent centre = ["clock"]
[bar.slots]
centre = ["+", "widget:right_of_clock"]   # → ["clock", "widget:right_of_clock"]
right  = ["widget:logo", "+"]              # prepend to the inherited list
```

`"+"` without `extends` (or extending a theme that has nothing in that slot) is
a hard error.

### `[tokens]` — the style knobs

Typed. A misspelled key is a hard error (it does **not** silently become a
custom value — see [extra.css](#extracss) for that). Every token has a default,
so you only set what you're changing.

| Token | Type | Default | Notes |
|---|---|---|---|
| `radius_bar` | int px | `8` | bar / segment corner radius |
| `radius_card` | int px | `8` | notification & popover cards |
| `radius_sm` | int px | `6` | chips, workspace pills |
| `radius_pill` | int px | `999` | OSD pill |
| `pad` | int px | `12` | interior padding baseline |
| `bg_alpha` | float `0.0–1.0` | `0.72` | bar / segment fill opacity (Hyprland blur shows through) |
| `light` | bool | `false` | flips the shell to ink-on-paper (light surfaces, dark text) |
| `bar_border` | enum | `full` | `full` (floating island, border all round) · `bottom` (flush edge-to-edge, hairline underneath) · `segmented` (transparent window, each slot-group is its own pill) |
| `spring` | cubic-bezier | `cubic-bezier(0.22, 1.35, 0.36, 1)` | overshoot curve — flips, pop-ins, trail draw |
| `spring_settle` | cubic-bezier | `cubic-bezier(0.22, 1.2, 0.36, 1)` | flatter curve — hovers, stat transitions |
| `accent_from` | palette slot | `accent` | active-workspace fill / gradient start |
| `accent_to` | palette slot | = `accent_from` | gradient end (Trail workspaces) |
| `accent2` | palette slot | = `accent_from` | second accent, independent of the workspace one (e.g. media equaliser) |
| `chip_height` | int px | `32` | bar chip / workspace-pill height |
| `icon_px` | int px | `24` | bar icon glyph size |
| `chip_gap` | int px | `0` | extra left-margin between bar chips |
| `font_family` | string | `Varela Round, sans-serif` | drives the **whole shell** (bar + launcher) **and** the shared ecosystem stylesheet — a bare face (`IBM Plex Sans`) or a list (`Outfit, sans-serif`) |
| `font_fallback` | string | `sans-serif` | fallback stack, appended and de-duped |
| `font_size_base` | int px | `14` | base font size for the bar + launcher |
| `font_weight` | int `100–900` | `400` | base font weight (`.bread-weight-*` on a widget node still overrides per element) |
| `sep_style` | enum | `line` | bar stat-chip divider — `line` · `none` · `dot` |
| `bar_shadow` | enum | `none` | drop shadow under the bar / segments — `none` · `soft` · `hard` |
| `ws_gradient_angle` | int deg | `90` | Trail workspace gradient direction (`accent_from → accent_to`) |
| `media_eq_style` | enum | `bars` | media-widget equaliser — `bars` (animated) · `none` |
| `osd_style` | enum | `pill` | OSD shape — `pill` (`radius_pill`) · `bar` (`radius_card`) |

`accent_*` values are **palette slot names, never hex** — see
[Palette slots](#palette-slots).

### `[bar.window]` — the bar's layer-shell geometry

```toml
[bar.window]
anchors   = ["top", "left", "right"]   # subset of top|bottom|left|right
width     = "fill"                       # "fill" or an int (px)
height    = 44                           # int px
margin    = { top = 12, left = 16, right = 16 }   # any of top/left/right/bottom
exclusive = "auto"                       # "auto" (reserve a strip) | "none" (float over content) | int px
keyboard  = "none"                       # none | on_demand | exclusive
layer     = "top"                        # top | overlay
```

- Anchoring to neither `left` nor `right` makes the surface **centred** (this
  is how `spotlight`'s capsule floats).
- `margin.bottom` is accepted but only meaningful for a `bottom`-anchored bar.
- `exclusive = "none"` + `keyboard = "on_demand"` is the launcher-capsule combo
  (see `spotlight`).

### `[bar.slots]` — what goes in the bar, left to right

Four ordered lists. Each entry is a **module name**, `widget:<key>`, or `"+"`.

```toml
[bar.slots]
left   = ["workspaces"]
centre = ["media", "clock"]
right  = ["cpu", "ram", "wifi", "battery", "control"]
drawer = ["launcher_results"]            # spotlight's expanding results area
```

Module names (`bread-theme describe` lists the current set):

```
workspaces  media  clock  volume  wifi  battery  control
cpu  ram  launcher_entry  launcher_results
```

A module that breadbar builds but that no slot list names is simply never
placed — omitting `media` from every list is how `glass-workbench` drops the
media widget. `tray` is not a slot entry; it lives in the control-panel
popover.

`widget:<key>` entries are anchor points for user Lua widgets
(`bread-shared`'s `WidgetPlacement`). The standard keys are
`right_of_workspaces`, `left_of_clock`, `right_of_clock`, `left_of_stats`. A
widget requesting a placement whose key appears in no slot list is dropped, so
carry the aliases you want to support even if the theme ships no widget itself.

### `[modules.workspaces]` / `[modules.clock]`

```toml
[modules.workspaces]
style      = "pill"        # trail | pill | dots
show_empty = true
dot_widths = [8, 13, 17, 22]   # dots only: px width for 0/1/2/3+ windows

[modules.clock]
style     = "plain"        # flip | plain | none
format    = "%H:%M"        # plain only
show_date = true           # plain only
placeholder_clock = true   # none only — the launcher entry's placeholder is the clock
```

**Changing `style` here forces a restart** (widget-type swap) — see
[Live switch vs. restart](#live-switch-vs-restart).

### `[launcher]`

```toml
[launcher]
mode = "overlay"           # overlay (breadbox window) | embedded (breadbar capsule)
```

`mode` is the only structural key. Everything else — `width`, `top`, `radius`,
`icon_px`, `row_radius`, `panel_alpha`, `selection_alpha`, … — is launcher CSS
read by breadbox; copy a builtin's block and adjust. `search_width` /
`search_radius` apply only to `mode = "embedded"`.

**Changing `mode` forces a restart.**

### `[surfaces."<namespace>"]` — the auxiliary windows

Keyed by layer-shell namespace: `breadbar-notif`, `breadbar-osd`,
`breadbar-panel`, `breadbar-dismiss` (and `breadbox`).

```toml
[surfaces."breadbar-notif"]
anchor = "top_right"       # top_right | bottom_right | bottom_centre | fill
offset = [16, 64]          # top_right/bottom_right: [horizontal, vertical];
                           # bottom_centre / fill: a single int; fill also takes [top, bottom]
width  = 320               # "fill" | "auto" | int px
layer  = "overlay"         # top | overlay
```

Rule of thumb for `offset`'s vertical value: your bar's far edge
(`margin + height`) plus an ~8–12px gap.

### `[compositor."<namespace>"]` — Hyprland layer rules

Appearance only — never placement, workspace, or focus.

```toml
[compositor."breadbar"]
blur         = true
ignore_alpha = 0.2         # only meaningful when blur = true
blur_popups  = true
animation    = "slide top" # or: no_anim = true
```

Blur *strength* is global (a known limitation) — this is on/off, `ignore_alpha`
and slide direction per namespace.

### `[panel]` — the control-panel popover

```toml
[panel]
min_width = 248                                  # CSS min-width of the panel body
sections  = ["volume", "output", "brightness", "system", "power", "tray"]
```

`sections` reorders / drops the control-panel's body sections; the `CONTROL`
header and caret are fixed at the top. A name left out is simply not shown.
Default = all six, in the order above.

**Changing `sections` forces a restart** (the body is assembled once at
launch). `min_width` is CSS → live.

### `[osd]` — the volume / brightness on-screen display

```toml
[osd]
enabled    = ["volume", "brightness"]   # which OSDs are active
dismiss_ms = 2000                        # how long a shown OSD lingers
```

The OSD's *position* is `[surfaces."breadbar-osd"]` and its *radius* is the
`osd_style` token; this table only adds which kinds run and for how long.

**Changing `enabled` forces a restart** (the watcher threads start at launch).
`dismiss_ms` applies to the next OSD.

### `[[bar.widget]]` — a live bar widget, no Lua

Declare a small widget inline: a poll command plus a node tree. It renders
through the same path as a `bread` daemon widget, wherever a `widget:<slot>`
entry sits in `[bar.slots]`.

```toml
[[bar.widget]]
id   = "loadavg"
slot = "right_of_clock"                  # must match a widget:<slot> in [bar.slots]
order = 0
bind = { cmd = "cut -d' ' -f1 /proc/loadavg", every = "5s" }   # or "500ms" / "2m"
node = { kind = "box", spacing = 4, class = "bread-chip", children = [
  { kind = "icon",  name = "cpu", size = 13 },
  { kind = "label", text = "{value}", color = "yellow", weight = "bold" },
] }
```

- `bind.cmd` runs under `sh -c` with **your** privileges (same trust as a
  `~/.config/bread` Lua module — no sandbox). Its trimmed stdout replaces
  every `{value}` in the tree on each `every` tick.
- `node.kind` is `box` (`orientation`, `spacing`, `class`, `children`),
  `label` (`text`, `class`, `color`, `weight`, `size`), `icon` (`name` |
  `path`, `size`, `class`), or `progress` (`value`, `class`).
- `color` ∈ `fg dim accent red green yellow blue pink teal`; `weight` ∈
  `normal bold`; `size` ∈ `xs sm md lg xl`. Bundled `icon` names include
  `cpu ram battery-* wifi-* bluetooth-* volume brightness lock sleep …`;
  anything else needs `path`.
- Tree limits: depth ≤ 4, ≤ 50 nodes.

**`[[bar.widget]]` changes apply live** — the poller set is rebuilt on a
theme switch, no restart.

### `css = "extra.css"`

Optional path (relative to the theme dir) to a raw-CSS file appended after the
rendered stylesheet. See below.

---

## Palette slots

Colours in a theme are **slot names**, not hex values. Each maps to a pywal
ANSI colour derived from the wallpaper, so the theme re-tints with the
wallpaper instead of clashing with it. BOS keeps `bg` / `fg` / `surface` /
`overlay` fixed; only `accent` and the six hues track the wallpaper.

```
bg  fg  surface  overlay  accent  red  green  yellow  blue  pink  teal
on-bg  on-surface  on-accent  on-red  on-overlay
```

In `[tokens]`, `accent_from = "yellow"` means "use pywal's yellow". In CSS
(base sheet or `extra.css`) the same slots are available as `@yellow`,
`@on-bg`, etc. — GTK resolves them per output. Writing a literal hex in a
theme breaks per-monitor theming; don't.

---

## Live switch vs. restart

When `shell.toml`'s `active` changes, or the active theme's directory is
edited on disk, breadbar applies as much as it can **without restarting**:

| Change | Applied |
|---|---|
| `[tokens]` (incl. fonts, chrome tokens), `extra.css`, colours, radii, springs, `light` | **live**, no blink |
| `[bar.window]` geometry (anchors, size, margin, exclusive zone, layer) | **live** |
| `[bar.slots]` order / membership · `[[bar.widget]]` | **live** |
| `[surfaces.*]` / `[compositor.*]` · `[panel].min_width` · `[osd].dismiss_ms` | **live** (next time it maps) |
| `[modules.workspaces].style` or `[modules.clock].style` | **restart** (one blink) |
| `[launcher].mode` · `[panel].sections` · `[osd].enabled` | **restart** |

The restart cases are structural — a widget-type swap, or a section / thread
set assembled once at launch. `breadbar::theme::needs_restart` is the exact
rule: `modules()`, `launcher().mode`, `panel().sections`, or `osd().enabled`
differ.

**Consequence for `extends`:** if your theme leaves `[modules]` and
`[launcher].mode` identical to another theme's, switching between the two is
fully live. `loaf` does this deliberately — it `extends = "glass-workbench"`
and never touches `[modules]`, so `glass-workbench ↔ loaf` is a live morph.

---

## extra.css

For a genuine one-off the token system can't express, add a `css` key and an
`extra.css` file:

```toml
css = "extra.css"
```

```css
/* ~/.config/bread/themes/mytheme/extra.css */
.control-panel-inner { box-shadow: 0 0 40px @accent; }
```

It's appended verbatim after the rendered base sheet, so it wins on
specificity ties. `@palette` slot names work. `{token}` placeholders do
**not** — but any key you put in `[tokens]` that isn't a known token is
carried through as a `{name}` substitution available to `extra.css` only:

```toml
[tokens]
my_glow_px = 40         # unknown token → ignored by the renderer, exposed to extra.css
```
```css
.control-panel-inner { box-shadow: 0 0 {my_glow_px}px @accent; }
```

A `{name}` in `extra.css` that matches neither a known token nor an `[tokens]`
key is a hard error at load — that's the typo guard.

---

## Testing & troubleshooting

| Command | Purpose |
|---|---|
| `bread-theme diagnose <id>` | exit 0 if `<id>` resolves; else print why and exit 1 |
| `bread-theme describe` | full `theme.toml` schema as JSON |
| `BREAD_SHELL_THEME=<id> breadbar --screenshot bar --output /tmp/bar.png` | render one theme's bar to a PNG without touching `shell.toml` (breadbar also has `--screenshot` views for `control-panel`, `notification`, `osd-volume`, …) |
| edit `shell.toml` → save | live-apply to the running shell (watch `breadbar`'s stderr for `applying in place` vs a restart) |

Common failures:

- **"CSS template references unknown name(s): …"** — a `{ref}` in your
  `extra.css` matched no token and no `[tokens]` key. Fix the spelling or add
  the key.
- **"modules.workspaces.style = "…" is not trail|pill|dots"** and similar —
  an enum typo. The message lists the valid values.
- **Shell shows `liquid-motion` after you set `active`** — the theme failed to
  load and fell back silently. Run `bread-theme diagnose <id>` to see the
  reason.
- **Colours look wrong / don't follow the wallpaper** — a literal hex where a
  palette slot name belongs.

---

## See also

- `bread-theme/assets/shell/*/theme.toml` — the five builtins, heavily
  commented, as reference implementations. `loaf` is the minimal one; `daylight`
  exercises every axis (light, bottom-anchored, segmented, blur-off).
- `docs/release-channels.md` — how a theme change in this repo reaches users.
