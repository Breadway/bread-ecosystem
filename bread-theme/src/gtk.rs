use gtk4::gio;
use gtk4::prelude::*;
use gtk4::CssProvider;
use std::cell::RefCell;
use std::path::Path;

thread_local! {
    static SHARED_PROVIDER: RefCell<Option<CssProvider>> = const { RefCell::new(None) };
    static SHARED_MONITOR:  RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static APP_PROVIDER:    RefCell<Option<CssProvider>> = const { RefCell::new(None) };
    static APP_MONITOR:     RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    #[allow(clippy::type_complexity)]
    static APP_BUILDER:     RefCell<Option<Box<dyn Fn() -> String>>> = const { RefCell::new(None) };
}

fn reload_shared() {
    let css = std::fs::read_to_string(crate::shared_css_path())
        .unwrap_or_else(|_| crate::render());
    SHARED_PROVIDER.with(|cell| apply_css(&css, cell));
}

fn reload_app() {
    let css = APP_BUILDER.with(|b| b.borrow().as_ref().map(|f| f()));
    if let Some(css) = css {
        APP_PROVIDER.with(|cell| apply_css(&css, cell));
    }
}

/// Watch the shared stylesheet for changes and run `reload` when it's rewritten.
///
/// `bread-theme` writes the file with write-tmp-then-rename (atomic), which
/// *replaces the inode*. A monitor on the file itself dies after the first
/// replace (inotify reports DELETE_SELF and never re-arms), so we monitor the
/// parent *directory* and filter for the stylesheet's filename — that fires
/// reliably on every reload. Returns the monitor (keep it alive to stay armed).
fn watch_theme_file(reload: fn()) -> Option<gio::FileMonitor> {
    let target = crate::shared_css_path();
    let dir = target.parent()?;
    // The dir must exist to be monitored; `bread-theme generate` makes it at
    // login, but create it here too so a GUI started first still arms the watch.
    let _ = std::fs::create_dir_all(dir);
    let monitor = gio::File::for_path(dir)
        .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        .ok()?;
    monitor.connect_changed(move |_, file, other, _event| {
        // The rename lands as an event whose file (or move destination) is the
        // stylesheet. Match either to catch both CREATED/CHANGED and MOVED_IN.
        let is_target = |f: &gio::File| f.path().as_deref() == Some(target.as_path());
        if is_target(file) || other.is_some_and(is_target) {
            reload();
        }
    });
    Some(monitor)
}

/// Apply an app's *own* stylesheet and keep it live across palette changes.
///
/// `build` is called now to produce the app-specific CSS, and again every time
/// the shared theme file is rewritten — i.e. whenever `bread-theme reload` (or
/// `generate`) runs after pywal changes. The app recolours in place, no restart.
///
/// This is the counterpart to [`apply_shared`]: that hot-reloads the *shared*
/// component sheet; this hot-reloads the app's *own* rules (which are built from
/// the palette, so they'd otherwise be frozen at startup). Apps that build their
/// CSS from [`crate::stylesheet`] themselves can use this alone; apps that layer
/// on top of [`apply_shared`] call both.
///
/// Call once at startup. The closure should read the current palette
/// ([`crate::load_palette`]) each time so it picks up the new colours.
pub fn apply_app_css<F: Fn() -> String + 'static>(build: F) {
    APP_BUILDER.with(|b| *b.borrow_mut() = Some(Box::new(build)));
    reload_app();
    APP_MONITOR.with(|cell| {
        if cell.borrow().is_some() {
            return;
        }
        *cell.borrow_mut() = watch_theme_file(reload_app);
    });
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
        *cell.borrow_mut() = watch_theme_file(reload_shared);
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
