//! Shared GTK4 layer-shell popup scaffold: the full-screen transparent
//! overlay window setup, `ListBox` up/down visible-row navigation, and
//! click-outside-to-close gesture were duplicated near-verbatim between
//! `breadbox/src/main.rs` and `breadclip/src/main.rs`:
//!
//! - Layer-shell window setup: `breadbox/src/main.rs:357-365` /
//!   `breadclip/src/main.rs:231-239` — identical `init_layer_shell` +
//!   namespace + `Layer::Overlay` + `KeyboardMode::Exclusive` + anchor all
//!   four edges + zero exclusive zone.
//! - Up/Down navigation loop: `breadbox/src/main.rs:515-546` /
//!   `breadclip/src/main.rs:423-454` — byte-for-byte identical "find the
//!   next/previous *visible* row" loop (breadclip's own comment even reads
//!   `// ---- Keyboard handler (capture phase, same as breadbox) ----`).
//! - Click-outside-close: `breadbox/src/main.rs:566-581` /
//!   `breadclip/src/main.rs:474-...` — identical bounds-check against a
//!   content widget (breadclip: `// ---- Click outside panel → close (same
//!   pattern as breadbox) ----`).
//!
//! Deliberately *not* extracted: the rest of each app's `EventControllerKey`
//! handling (Enter/Delete semantics, filter chips, search) — those differ
//! per app (`do_launch` vs `do_copy`+`Delete`-to-remove) and forcing them
//! into one callback-owning "scaffold" struct would be a leakier
//! abstraction than the ~5 free functions below.
//!
//! Requires the `gtk` feature.

use gtk4::prelude::*;
use gtk4::{ApplicationWindow, GestureClick};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

/// Build the full-screen transparent overlay window every layer-shell popup
/// in this ecosystem starts from: layered above normal windows, keyboard-
/// exclusive (so Escape/Enter/arrow keys reach the popup instead of the
/// focused client behind it), anchored to all four edges with zero
/// exclusive zone (so it doesn't reserve screen space or push other layer
/// clients around).
pub fn new_overlay_window(app: &gtk4::Application, namespace: &str) -> ApplicationWindow {
    let window = ApplicationWindow::builder().application(app).build();
    window.init_layer_shell();
    window.set_namespace(Some(namespace));
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    window.set_exclusive_zone(0);
    window
}

/// Select the next *visible* row after the current selection (rows can be
/// hidden by a live search filter — a plain "select index + 1" would land
/// on a filtered-out row). No-op if there is no next visible row.
pub fn select_next_visible(list: &gtk4::ListBox) {
    let cur = list.selected_row().map(|r| r.index()).unwrap_or(-1);
    let mut i = cur + 1;
    loop {
        match list.row_at_index(i) {
            Some(r) if r.is_visible() => {
                list.select_row(Some(&r));
                break;
            }
            Some(_) => i += 1,
            None => break,
        }
    }
}

/// Select the previous *visible* row before the current selection. No-op if
/// there is no previous visible row.
pub fn select_prev_visible(list: &gtk4::ListBox) {
    let cur = list.selected_row().map(|r| r.index()).unwrap_or(0);
    let mut i = cur - 1;
    loop {
        if i < 0 {
            break;
        }
        match list.row_at_index(i) {
            Some(r) if r.is_visible() => {
                list.select_row(Some(&r));
                break;
            }
            Some(_) => i -= 1,
            None => break,
        }
    }
}

/// Attach a click gesture to `window` that calls `on_outside` whenever a
/// click lands outside `content`'s bounds (e.g. clicking the transparent
/// full-screen backdrop around a centered launcher/panel widget).
pub fn close_on_outside_click(
    window: &ApplicationWindow,
    content: &impl IsA<gtk4::Widget>,
    on_outside: impl Fn() + 'static,
) {
    let content = content.clone().upcast::<gtk4::Widget>();
    let win_ref = window.clone();
    let gesture = GestureClick::new();
    gesture.connect_pressed(move |_, _, x, y| {
        if let Some(b) = content.compute_bounds(&win_ref) {
            if x < b.x() as f64
                || x > (b.x() + b.width()) as f64
                || y < b.y() as f64
                || y > (b.y() + b.height()) as f64
            {
                on_outside();
            }
        }
    });
    window.add_controller(gesture);
}
