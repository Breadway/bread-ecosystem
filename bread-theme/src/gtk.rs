use gtk4::gdk::prelude::*;
use gtk4::gio;
use gtk4::glib::object::ObjectType;
use gtk4::prelude::*;
use gtk4::CssProvider;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use crate::Palette;

/// Per-widget app-CSS builder: given the resolved palette for the widget's
/// monitor, produce the app stylesheet to layer on top of the theme CSS.
type AppCssBuilder = Rc<dyn Fn(&Palette) -> String>;

/// Above APPLICATION (600) so we beat [`apply_shared`], below USER (800)
/// so `apply_user_css` still wins.
const BIND_PRIORITY: u32 = gtk4::STYLE_PROVIDER_PRIORITY_USER - 10;

thread_local! {
    static SHARED_PROVIDER: RefCell<Option<CssProvider>> = const { RefCell::new(None) };
    static SHARED_MONITOR:  RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static APP_PROVIDER:    RefCell<Option<CssProvider>> = const { RefCell::new(None) };
    static APP_MONITOR:     RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    #[allow(clippy::type_complexity)]
    static APP_BUILDER:     RefCell<Option<Box<dyn Fn() -> String>>> = const { RefCell::new(None) };
}

fn reload_shared() {
    let css = std::fs::read_to_string(crate::shared_css_path()).unwrap_or_else(|_| crate::render());
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

/// A filter/tag chip using the shared `.chip` stylesheet rule (an
/// `@overlay`-filled pill, `@accent`-filled when the `active` CSS class is
/// set) instead of a fresh literal color — this is the fix for the same
/// component drifting to three different fills across breadclip (grey),
/// breadpad, and breadman (both cream), none of which agreed with each
/// other or with the shared token.
pub fn chip(label: &str) -> gtk4::Button {
    gtk4::Button::builder()
        .label(label)
        .css_classes(["chip"])
        .build()
}

/// Toggles a chip's (or any widget's) `active` CSS class — the `.chip.active`
/// stylesheet rule fills it with the accent instead of the neutral overlay.
/// Wiring *when* a chip becomes active (single-select filter, multi-select
/// tags, etc.) is genuinely per-app, so that stays the caller's job; this is
/// just the one-line visual toggle every case needs.
pub fn set_chip_active(chip: &impl IsA<gtk4::Widget>, active: bool) {
    if active {
        chip.add_css_class("active");
    } else {
        chip.remove_css_class("active");
    }
}

/// Gdk connector for the monitor currently showing this widget, if any.
pub fn output_for_widget(widget: &impl IsA<gtk4::Widget>) -> Option<String> {
    let widget = widget.as_ref();
    let native = widget.native()?;
    let surface = NativeExt::surface(&native)?;
    let monitor = widget.display().monitor_at_surface(&surface)?;
    monitor.connector().map(|c| c.to_string())
}

struct WidgetBind {
    output: String,
    theme: CssProvider,
    app: Option<CssProvider>,
    app_build: Option<AppCssBuilder>,
    /// Keep the directory monitor + child model alive for this widget.
    _watch: Option<gio::ListModel>,
}

thread_local! {
    static BINDS: RefCell<HashMap<usize, WidgetBind>> = RefCell::new(HashMap::new());
    static THEMES_MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static DESTROY_HOOKED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    static AUTO_HOOKED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    static ENTER_HOOKED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
}

fn widget_key(widget: &gtk4::Widget) -> usize {
    widget.as_ptr() as usize
}

#[allow(deprecated)]
fn add_widget_provider(widget: &gtk4::Widget, provider: &CssProvider, prio: u32) {
    widget.style_context().add_provider(provider, prio);
}

/// Same `CssProvider` on the widget and its current descendants so component
/// rules actually reach buttons/labels (a style-context provider is not
/// inherited by children).
fn attach_tree(widget: &gtk4::Widget, theme: &CssProvider, app: Option<&CssProvider>) {
    add_widget_provider(widget, theme, BIND_PRIORITY);
    if let Some(app) = app {
        add_widget_provider(widget, app, BIND_PRIORITY + 1);
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        attach_tree(&c, theme, app);
        child = c.next_sibling();
    }
}

fn ensure_destroy_cleanup(widget: &gtk4::Widget) {
    let key = widget_key(widget);
    let inserted = DESTROY_HOOKED.with(|s| s.borrow_mut().insert(key));
    if !inserted {
        return;
    }
    widget.connect_destroy(move |w| {
        let key = widget_key(w);
        BINDS.with(|b| {
            b.borrow_mut().remove(&key);
        });
        DESTROY_HOOKED.with(|s| {
            s.borrow_mut().remove(&key);
        });
        AUTO_HOOKED.with(|s| {
            s.borrow_mut().remove(&key);
        });
    });
}

fn ensure_themes_watch() {
    THEMES_MONITOR.with(|cell| {
        if cell.borrow().is_some() {
            return;
        }
        let dir = crate::themes_dir();
        let _ = std::fs::create_dir_all(&dir);
        let monitor = gio::File::for_path(&dir)
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
            .ok();
        if let Some(ref m) = monitor {
            m.connect_changed(move |_, file, other, _event| {
                let path = file.path().or_else(|| other.and_then(|f| f.path()));
                let Some(path) = path else {
                    return;
                };
                if path.extension().and_then(|e| e.to_str()) != Some("css") {
                    return;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    return;
                };
                reload_binds_for_sanitized(stem);
            });
        }
        *cell.borrow_mut() = monitor;
    });
}

fn reload_binds_for_sanitized(sanitized: &str) {
    BINDS.with(|binds| {
        for bind in binds.borrow_mut().values_mut() {
            if crate::sanitize_output(&bind.output) != sanitized {
                continue;
            }
            let palette = crate::load_palette_for(&bind.output);
            bind.theme
                .load_from_string(&crate::stylesheet_resolved(&palette));
            if let (Some(build), Some(provider)) = (&bind.app_build, &bind.app) {
                provider.load_from_string(&crate::resolve_color_names(&build(&palette), &palette));
            }
        }
    });
}

fn watch_root_children(widget: &gtk4::Widget) -> gio::ListModel {
    let model = widget.observe_children();
    let root = widget.downgrade();
    model.connect_items_changed(move |_, _, _, _| {
        let Some(root) = root.upgrade() else {
            return;
        };
        let key = widget_key(&root);
        BINDS.with(|binds| {
            if let Some(bind) = binds.borrow().get(&key) {
                attach_tree(&root, &bind.theme, bind.app.as_ref());
            }
        });
    });
    model
}

fn bind_window_inner(
    widget: &gtk4::Widget,
    output: &str,
    app_build: Option<AppCssBuilder>,
) {
    let key = widget_key(widget);
    let palette = crate::load_palette_for(output);
    let theme_css = crate::stylesheet_resolved(&palette);
    let app_css = app_build
        .as_ref()
        .map(|build| crate::resolve_color_names(&build(&palette), &palette));

    BINDS.with(|binds| {
        let mut map = binds.borrow_mut();
        if let Some(existing) = map.get_mut(&key) {
            existing.output = output.to_string();
            existing.theme.load_from_string(&theme_css);
            existing.app_build = app_build.clone();
            match (&app_css, existing.app.as_ref()) {
                (Some(css), Some(p)) => p.load_from_string(css),
                (Some(css), None) => {
                    let p = CssProvider::new();
                    p.load_from_string(css);
                    add_widget_provider(widget, &p, BIND_PRIORITY + 1);
                    existing.app = Some(p);
                }
                (None, Some(p)) => p.load_from_string(""),
                (None, None) => {}
            }
            attach_tree(widget, &existing.theme, existing.app.as_ref());
            return;
        }

        let theme = CssProvider::new();
        theme.load_from_string(&theme_css);
        add_widget_provider(widget, &theme, BIND_PRIORITY);

        let app = app_css.map(|css| {
            let p = CssProvider::new();
            p.load_from_string(&css);
            add_widget_provider(widget, &p, BIND_PRIORITY + 1);
            p
        });

        attach_tree(widget, &theme, app.as_ref());

        let child_model = watch_root_children(widget);
        map.insert(
            key,
            WidgetBind {
                output: output.to_string(),
                theme,
                app,
                app_build,
                _watch: Some(child_model),
            },
        );
    });

    ensure_destroy_cleanup(widget);
    ensure_themes_watch();
    ensure_map_reattach(widget);
}

fn ensure_map_reattach(widget: &gtk4::Widget) {
    // `connect_map` once per widget — re-bind already lives in BINDS.
    thread_local! {
        static MAP_HOOKED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    }
    let key = widget_key(widget);
    let inserted = MAP_HOOKED.with(|s| s.borrow_mut().insert(key));
    if !inserted {
        return;
    }
    widget.connect_map(|w| {
        BINDS.with(|binds| {
            if let Some(bind) = binds.borrow().get(&widget_key(w)) {
                attach_tree(w, &bind.theme, bind.app.as_ref());
            }
        });
    });
    widget.connect_destroy(move |_| {
        MAP_HOOKED.with(|s| {
            s.borrow_mut().remove(&key);
        });
    });
}

/// Attach a widget-level `CssProvider` with
/// `stylesheet_resolved(load_palette_for(output))` above APPLICATION so it
/// beats [`apply_shared`] for this widget tree. User CSS still wins.
/// Calling again on the same widget replaces the provider; it does not stack.
pub fn bind_window(widget: &impl IsA<gtk4::Widget>, output: &str) {
    bind_window_inner(widget.as_ref(), output, None);
}

/// [`bind_window`], then also apply `build(&palette)` on the same widget.
/// App CSS may still use `@accent` etc.; those names are inlined against
/// the same palette before loading.
pub fn bind_window_with_app_css<F>(widget: &impl IsA<gtk4::Widget>, output: &str, build: F)
where
    F: Fn(&Palette) -> String + 'static,
{
    bind_window_inner(widget.as_ref(), output, Some(Rc::new(build)));
}

fn attach_enter_monitor(widget: &gtk4::Widget, build: Option<AppCssBuilder>) {
    let Some(native) = widget.native() else {
        return;
    };
    let Some(surface) = NativeExt::surface(&native) else {
        return;
    };
    let surf_key = surface.as_ptr() as usize;
    let already = ENTER_HOOKED.with(|s| !s.borrow_mut().insert(surf_key));
    if already {
        return;
    }
    let widget = widget.clone();
    surface.connect_enter_monitor(move |_, monitor| {
        let Some(conn) = monitor.connector() else {
            return;
        };
        bind_window_inner(&widget, conn.as_str(), build.clone());
    });
}

fn bind_auto(native: &gtk4::Native, build: Option<AppCssBuilder>) {
    let widget = native.upcast_ref::<gtk4::Widget>().clone();

    let apply = {
        let widget = widget.clone();
        let build = build.clone();
        Rc::new(move || {
            if let Some(output) = output_for_widget(&widget) {
                bind_window_inner(&widget, &output, build.clone());
            }
        })
    };

    apply();

    let key = widget_key(&widget);
    let inserted = AUTO_HOOKED.with(|s| s.borrow_mut().insert(key));
    if inserted {
        widget.connect_realize({
            let apply = apply.clone();
            let widget = widget.clone();
            let build = build.clone();
            move |_| {
                apply();
                attach_enter_monitor(&widget, build.clone());
            }
        });
        widget.connect_map({
            let apply = apply.clone();
            move |_| apply()
        });
        ensure_destroy_cleanup(&widget);
    }

    if widget.is_realized() {
        attach_enter_monitor(&widget, build);
    }
}

/// Realize + `GdkSurface::enter-monitor`: rebind when the window moves
/// outputs. If the connector is unknown, leave unbound (display fallback)
/// rather than guessing the wrong monitor.
pub fn bind_window_auto(window: &impl IsA<gtk4::Native>) {
    bind_auto(window.as_ref(), None);
}

/// [`bind_window_auto`] plus per-output app CSS, resolved to hex.
pub fn bind_window_auto_with_app_css<F>(window: &impl IsA<gtk4::Native>, build: F)
where
    F: Fn(&Palette) -> String + 'static,
{
    bind_auto(window.as_ref(), Some(Rc::new(build)));
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
