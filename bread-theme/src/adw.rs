//! Composite libadwaita widgets for the bread ecosystem's design system —
//! the actual mechanism (real GNOME-style widgets, not more hand-rolled CSS)
//! behind why bos-settings' sidebar/section/toggle rows read as more polished
//! than the plain-GTK4 apps'. An app calls these instead of assembling boxes
//! and labels and raw widgets from scratch each time, so spacing/sizing/
//! grouping decisions get made once, correctly, here — not re-derived per
//! screen.
//!
//! Not usable from the five `gtk4-layer-shell` apps (breadbar, breadbox,
//! breadclip, breadsearch, breadpad): `AdwApplicationWindow`'s own chrome
//! isn't compatible with a layer-shell surface, and these helpers assume an
//! ordinary top-level window. Apps with a plain top-level window (breadman,
//! breadhelp) can use the full set.

use libadwaita as adw;
use adw::prelude::*;

/// Call once at startup, before building any widgets from this module —
/// initializes libadwaita's style manager and forces dark mode regardless of
/// the system GTK theme preference. bread-theme's whole design is a *fixed*
/// dark base (only the accent tracks pywal — see `palette::FIXED_BACKGROUND`
/// etc.) so an app respecting a light system preference here would silently
/// break that contract the moment someone's GNOME settings say "light".
pub fn init() {
    adw::init().expect("failed to initialize libadwaita");
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
}

/// A titled, optionally-described group of setting rows — the
/// title-then-description-then-rows rhythm bos-settings already uses per
/// section, now available to native GTK4/relm4 apps instead of a hand-rolled
/// vbox with a bold label glued to the top.
pub fn preferences_group(title: &str, description: Option<&str>) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title(title).build();
    if let Some(desc) = description {
        group.set_description(Some(desc));
    }
    group
}

/// A single on/off setting row with a correctly-sized, correctly-positioned
/// switch — the direct fix for the ~1400px-wide stretched-switch bug
/// (breadman/settings had no intrinsic width on its hand-rolled switch, so
/// it filled the row like a progress bar).
pub fn toggle_row(title: &str, subtitle: Option<&str>, active: bool) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).active(active).build();
    if let Some(sub) = subtitle {
        row.set_subtitle(sub);
    }
    row
}

/// A single numeric setting row (spin button docked to its own label,
/// instead of stranded ~1300px away at the window's far edge).
pub fn spin_row(title: &str, subtitle: Option<&str>, adjustment: &gtk4::Adjustment) -> adw::SpinRow {
    let row = adw::SpinRow::builder().title(title).adjustment(adjustment).build();
    if let Some(sub) = subtitle {
        row.set_subtitle(sub);
    }
    row
}

/// A general label(+subtitle) row with room for a trailing widget
/// (`row.add_suffix(&widget)`) — for settings that don't fit switch/spin
/// (text entries, buttons, dropdowns, a raw value display).
pub fn action_row(title: &str, subtitle: Option<&str>) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).build();
    if let Some(sub) = subtitle {
        row.set_subtitle(sub);
    }
    row
}

/// A page of one or more `preferences_group`s, with correct margins and
/// scroll handling — the top-level content container for a settings screen.
pub fn preferences_page() -> adw::PreferencesPage {
    adw::PreferencesPage::new()
}
