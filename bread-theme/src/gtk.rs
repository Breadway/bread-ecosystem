use gtk4::gio;
use gtk4::prelude::*;
use gtk4::CssProvider;
use std::cell::RefCell;
use std::path::Path;

thread_local! {
    static SHARED_PROVIDER: RefCell<Option<CssProvider>> = const { RefCell::new(None) };
    static SHARED_MONITOR:  RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
}

fn reload_shared() {
    let css = std::fs::read_to_string(crate::shared_css_path())
        .unwrap_or_else(|_| crate::render());
    SHARED_PROVIDER.with(|cell| apply_css(&css, cell));
}

/// Load the ecosystem's shared stylesheet (the file written by
/// `bread-theme generate`, or a freshly rendered fallback if absent) at
/// APPLICATION priority, and watch the file so the whole UI recolours live when
/// the palette changes — no app rebuild or restart needed.
///
/// Call once at startup; then add the app's own CSS provider *after* this so
/// app-specific rules win on equal specificity.
pub fn apply_shared() {
    reload_shared();
    SHARED_MONITOR.with(|cell| {
        if cell.borrow().is_some() {
            return;
        }
        let file = gio::File::for_path(crate::shared_css_path());
        if let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            monitor.connect_changed(|_, _, _, _| reload_shared());
            *cell.borrow_mut() = Some(monitor);
        }
    });
}

/// Apply a CSS string to the default display at APPLICATION priority.
/// Re-uses an existing provider if one is passed in (for SIGHUP reloads).
pub fn apply_css(css: &str, provider: &RefCell<Option<CssProvider>>) {
    let display = gtk4::gdk::Display::default().expect("no display");
    let mut guard = provider.borrow_mut();
    if let Some(p) = guard.as_ref() {
        p.load_from_string(css);
    } else {
        let p = CssProvider::new();
        p.load_from_string(css);
        gtk4::style_context_add_provider_for_display(
            &display,
            &p,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        *guard = Some(p);
    }
}

/// Apply a user CSS override file at USER priority. Clears the provider if the
/// file is absent so stale overrides don't persist across SIGHUP reloads.
pub fn apply_user_css(path: &Path, provider: &RefCell<Option<CssProvider>>) {
    let display = gtk4::gdk::Display::default().expect("no display");
    let mut guard = provider.borrow_mut();
    match std::fs::read_to_string(path) {
        Ok(css) => {
            if let Some(p) = guard.as_ref() {
                p.load_from_string(&css);
            } else {
                let p = CssProvider::new();
                p.load_from_string(&css);
                gtk4::style_context_add_provider_for_display(
                    &display,
                    &p,
                    gtk4::STYLE_PROVIDER_PRIORITY_USER,
                );
                *guard = Some(p);
            }
        }
        Err(_) => {
            if let Some(p) = guard.as_ref() {
                p.load_from_string("");
            }
        }
    }
}
