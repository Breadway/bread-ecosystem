# Bread Design System

Unified visual identity for breadbar, breadbox, breadpad/breadman, and
bos-settings.

## Architecture (single source of truth)

The tokens below are implemented once in the **`bread-theme`** crate as
`stylesheet(&Palette)` — the full component stylesheet (buttons, entries,
switches, lists/rows/sidebars, cards, chips, scrollbars, headings) over a
canonical `@define-color` palette (`surface`=color0, `overlay`=color7,
`accent`=color4).

- The `bread-theme` **CLI** renders it from the live pywal palette to
  `$XDG_RUNTIME_DIR/bread/theme.css` (run at login and from a pywal hook).
- Every GUI loads that file via `bread_theme::gtk::apply_shared()` and
  **live-reloads** it, then layers on only its own app-specific rules.

Result: one definition, no per-app drift, and palette changes recolour the
whole desktop with no rebuilds. Apps reference the shared `@define-color`
names rather than raw palette slots.

## Typography

- **Font Family**: Varela Round, sans-serif
- **Base Size**: 14px
- **Secondary**: 12px (metadata, helper text, secondary labels)
- **Font Weight**: Normal (400) for body, Bold (700) for emphasis

## Spacing Scale (4px units)

Use these values consistently across all projects:

- **xs**: 4px (small gaps, internal padding)
- **sm**: 8px (default spacing between elements)
- **md**: 12px (medium spacing, main padding)
- **lg**: 16px (large padding, major spacing)
- **xl**: 20px (extra large spacing, section breaks)

## Border Radius

Establish a visual hierarchy with consistent rounding:

- **Primary** (buttons, cards, main containers): **8px**
- **Secondary** (input fields, chips, entries): **6px**
- **Tertiary** (small interactive elements): **4px**
- **Pill** (fully rounded buttons, badges): **999px**

## Color System

All projects use **pywal dynamic theming** with **Catppuccin Mocha** as the fallback palette:

- **Background**: `#1e1e2e` (Catppuccin)
- **Foreground**: `#cdd6f4` (Catppuccin)
- **Surface**: `#181825` (Catppuccin)
- **Accent**: Dynamic (from pywal)

Color palette slots (via wal):
- color0–color7: ANSI colors
- Semantic: red, green, yellow, blue, pink, teal

## Component Standards

### Buttons
- Border Radius: 8px
- Padding: 8px 16px (primary), 4px 8px (secondary)
- Font Size: 14px
- Background: Theme accent color

### Input Fields
- Border Radius: 6px
- Padding: 12px 16px
- Font Size: 14px
- Border: 1px or 2px solid (blue on focus)

### Cards
- Border Radius: 8px
- Padding: 12px
- Margin: 8px
- Box Shadow: Optional, for depth

### Stat Labels
- Font Size: 14px
- Margin Right (between icon/text): 5px
- Group Margin Right: 12px

### Notification Cards
- Border Radius: 8px
- Padding: 12px
- Margin Bottom: 8px
- Font Size: 14px (summary), 12px (body)

## Current Implementation

All GUI apps load `bread_theme::stylesheet` (via the generated shared file) and
add only app-specific rules:

- **breadbar** — shared base + bar window, workspace buttons, stats, notification
  and OSD cards.
- **breadbox** — shared base + launcher panel, search entry, result rows.
- **breadpad / breadman** — shared base + capture popup, type chips, note cards,
  reminder window, sidebar rows.
- **bos-settings** — shared base + content padding only (was previously a
  hardcoded Nord palette; migrated to the shared stylesheet).
- **breadcrumbs** — CLI tool; ANSI colours only, no GUI styling.

> Palette note: the fallback is Catppuccin Mocha, but installs (e.g. BOS) drive
> the real palette from pywal — BOS ships a black-base palette.

## Future Consistency Checks

When adding new components or updating existing ones:
1. Use Varela Round for all text
2. Set base font size to 14px (12px for secondary)
3. Use spacing scale (4px units: 4, 8, 12, 16, 20)
4. Use border radius from this system (8px default, 6px secondary)
5. Leverage pywal colors for dynamic theming
6. Keep margins/padding consistent across similar components
