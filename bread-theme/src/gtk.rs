use gtk4::CssProvider;
use std::cell::RefCell;
use std::path::Path;

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
