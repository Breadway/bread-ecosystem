use gtk4::gdk::prelude::*;
use gtk4::gio;
use gtk4::glib;
use gtk4::glib::object::ObjectType;
use gtk4::prelude::*;
use gtk4::CssProvider;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::thread::LocalKey;

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
    static SHARED_RETRY:    Cell<bool> = const { Cell::new(false) };
    static APP_PROVIDER:    RefCell<Option<CssProvider>> = const { RefCell::new(None) };
    static APP_MONITOR:     RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static APP_RETRY:       Cell<bool> = const { Cell::new(false) };
    #[allow(clippy::type_complexity)]
    static APP_BUILDER:     RefCell<Option<Box<dyn Fn() -> String>>> = const { RefCell::new(None) };
}

/// Arm a directory `FileMonitor` into `slot`, and if `monitor_directory` fails
/// (a dir race at login, a transient permissions hiccup) schedule a **bounded**
/// lazy retry instead of giving up for the whole session.
///
/// The pre-fix code stored `None` on failure; the watch was only ever re-armed
/// as a side effect of another `bind_window*` call, so a consumer that binds
/// exactly once (e.g. `bread-polkit`) lost per-monitor reload permanently after
/// a single failure. `guard` stops a burst of `bind_window*` calls from each
/// spawning their own retry timer.
fn arm_or_retry(
    slot: &'static LocalKey<RefCell<Option<gio::FileMonitor>>>,
    guard: &'static LocalKey<Cell<bool>>,
    arm: fn() -> Option<gio::FileMonitor>,
) {
    if slot.with(|c| c.borrow().is_some()) {
        return;
    }
    if let Some(m) = arm() {
        slot.with(|c| *c.borrow_mut() = Some(m));
        return;
    }
    if guard.with(|g| g.replace(true)) {
        return; // a retry loop is already ticking
    }
    let mut attempts: u32 = 0;
    gtk4::glib::timeout_add_seconds_local(2, move || {
        attempts += 1;
        let armed = slot.with(|c| c.borrow().is_some())
            || match arm() {
                Some(m) => {
                    slot.with(|c| *c.borrow_mut() = Some(m));
                    true
                }
                None => false,
            };
        if armed || attempts >= 5 {
            guard.with(|g| g.set(false));
            gtk4::glib::ControlFlow::Break
        } else {
            gtk4::glib::ControlFlow::Continue
        }
    });
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
    arm_or_retry(&APP_MONITOR, &APP_RETRY, || watch_theme_file(reload_app));
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
    arm_or_retry(&SHARED_MONITOR, &SHARED_RETRY, || {
        watch_theme_file(reload_shared)
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

/// Every `observe_children` model in a bound widget's current subtree, kept
/// alive together (drop them all when the bind is forgotten). Shared so the
/// recursive [`hook_subtree`] can push newly-discovered descendant containers
/// in from inside an `items-changed` closure.
type SubtreeWatches = Rc<RefCell<Vec<gio::ListModel>>>;

struct WidgetBind {
    output: String,
    theme: CssProvider,
    app: Option<CssProvider>,
    app_build: Option<AppCssBuilder>,
    /// Keep every subtree child-model alive for this widget.
    _watch: SubtreeWatches,
}

thread_local! {
    static BINDS: RefCell<HashMap<usize, WidgetBind>> = RefCell::new(HashMap::new());
    static THEMES_MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static THEMES_RETRY: Cell<bool> = const { Cell::new(false) };
    static PALETTES_MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static PALETTES_RETRY: Cell<bool> = const { Cell::new(false) };
    static DESTROY_HOOKED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    static AUTO_HOOKED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    static MAP_HOOKED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    /// Surfaces whose `enter-monitor` we've already hooked, keyed by
    /// `GdkSurface` pointer (not widget pointer — a widget can be re-realized
    /// onto a fresh surface).
    static ENTER_HOOKED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    /// widget pointer -> the surface pointer currently in [`ENTER_HOOKED`] for
    /// it, so [`forget_bind`] can clear the enter hook by the right key when
    /// the widget is destroyed.
    static ENTER_SURFACE: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
}

/// Drop every thread-local record for a destroyed bound widget.
///
/// `DESTROY_HOOKED`/`AUTO_HOOKED`/`MAP_HOOKED`/`BINDS` are keyed by the widget
/// pointer; `ENTER_HOOKED` is keyed by the *surface* pointer, so we look the
/// surface up via `ENTER_SURFACE`. Leaving the `ENTER_HOOKED` entry behind used
/// to mean a later window whose surface was allocated at the same address would
/// hit the `already` early-return in [`attach_enter_monitor`] and silently
/// never re-theme when moved between monitors.
fn forget_bind(key: usize) {
    BINDS.with(|b| {
        b.borrow_mut().remove(&key);
    });
    DESTROY_HOOKED.with(|s| {
        s.borrow_mut().remove(&key);
    });
    AUTO_HOOKED.with(|s| {
        s.borrow_mut().remove(&key);
    });
    MAP_HOOKED.with(|s| {
        s.borrow_mut().remove(&key);
    });
    if let Some(surf_key) = ENTER_SURFACE.with(|m| m.borrow_mut().remove(&key)) {
        ENTER_HOOKED.with(|s| {
            s.borrow_mut().remove(&surf_key);
        });
    }
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
    widget.connect_destroy(move |w| forget_bind(widget_key(w)));
}

/// `themes/<stem>.<ext>` (or `palettes/<stem>.<ext>`) → the file stem, iff the
/// event path actually carries `want_ext`. Factored out so the reload-routing
/// logic is unit-testable without a `FileMonitor`.
fn reload_stem_for_event(path: &Path, want_ext: &str) -> Option<String> {
    if path.extension().and_then(|e| e.to_str()) != Some(want_ext) {
        return None;
    }
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

fn arm_dir_reload_watch(dir: std::path::PathBuf, want_ext: &'static str) -> Option<gio::FileMonitor> {
    let _ = std::fs::create_dir_all(&dir);
    let monitor = gio::File::for_path(&dir)
        .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        .ok()?;
    monitor.connect_changed(move |_, file, other, _event| {
        let path = file.path().or_else(|| other.and_then(|f| f.path()));
        let Some(stem) = path.as_deref().and_then(|p| reload_stem_for_event(p, want_ext)) else {
            return;
        };
        reload_binds_for_sanitized(&stem);
    });
    Some(monitor)
}

fn try_arm_themes_watch() -> Option<gio::FileMonitor> {
    arm_dir_reload_watch(crate::themes_dir(), "css")
}

/// Watch `themes/<output>.css` so a per-output pywal regenerate recolours every
/// window bound to that output in place. Bounded lazy retry: a transient
/// `monitor_directory` failure no longer kills per-monitor reload for the
/// session (findings #5).
fn ensure_themes_watch() {
    arm_or_retry(&THEMES_MONITOR, &THEMES_RETRY, try_arm_themes_watch);
}

fn try_arm_palettes_watch() -> Option<gio::FileMonitor> {
    arm_dir_reload_watch(crate::palettes_dir(), "json")
}

/// Also watch `palettes/<output>.json` (finding #6): the CLI writes the `.json`
/// and the `.css` together, but third-party tooling that only rewrites the
/// palette JSON would otherwise leave every bound window stale — the reload
/// path re-reads the JSON via `load_palette_for` regardless, so reacting to
/// either file is correct (a paired write just reloads twice, harmlessly).
fn ensure_palettes_watch() {
    arm_or_retry(&PALETTES_MONITOR, &PALETTES_RETRY, try_arm_palettes_watch);
}

fn reload_binds_for_sanitized(sanitized: &str) {
    let ty = crate::Typography::active();
    BINDS.with(|binds| {
        for bind in binds.borrow_mut().values_mut() {
            if crate::sanitize_output(&bind.output) != sanitized {
                continue;
            }
            let palette = crate::load_palette_for(&bind.output);
            bind.theme
                .load_from_string(&crate::stylesheet_resolved(&ty, &palette));
            if let (Some(build), Some(provider)) = (&bind.app_build, &bind.app) {
                provider.load_from_string(&crate::resolve_color_names(&build(&palette), &palette));
            }
        }
    });
}

/// Re-run [`attach_tree`] over the whole bound subtree of `root`.
fn reattach_bound_tree(root: &gtk4::Widget) {
    let key = widget_key(root);
    BINDS.with(|binds| {
        if let Some(bind) = binds.borrow().get(&key) {
            attach_tree(root, &bind.theme, bind.app.as_ref());
        }
    });
}

/// Recursively hook `observe_children` on `widget` *and every current
/// descendant container*, so a subtree added lazily at **any** depth after the
/// bind — a popover's contents, menu items, rows appended into an existing box
/// — gets the per-output provider too (finding #3).
///
/// The pre-fix code only watched the root's *direct* children, so anything
/// deeper than one level that appeared after bind/map silently rendered with
/// the display-wide shared sheet (the wrong monitor's colours). Every model is
/// parked in `watches` (which the `WidgetBind` owns) so the whole set is
/// dropped together when the bind is forgotten; newly-appeared containers are
/// hooked in from inside the `items-changed` closure.
fn hook_subtree(widget: &gtk4::Widget, root: &glib::WeakRef<gtk4::Widget>, watches: &SubtreeWatches) {
    let model = widget.observe_children();
    {
        let root = root.clone();
        let watches = watches.clone();
        model.connect_items_changed(move |model, pos, _removed, added| {
            let Some(root_w) = root.upgrade() else {
                return;
            };
            reattach_bound_tree(&root_w);
            for i in pos..pos + added {
                if let Some(child) = model.item(i).and_downcast::<gtk4::Widget>() {
                    hook_subtree(&child, &root, &watches);
                }
            }
        });
    }
    watches.borrow_mut().push(model);
    let mut child = widget.first_child();
    while let Some(c) = child {
        hook_subtree(&c, root, watches);
        child = c.next_sibling();
    }
}

fn watch_subtree(widget: &gtk4::Widget) -> SubtreeWatches {
    let watches: SubtreeWatches = Rc::new(RefCell::new(Vec::new()));
    hook_subtree(widget, &widget.downgrade(), &watches);
    watches
}

fn bind_window_inner(
    widget: &gtk4::Widget,
    output: &str,
    app_build: Option<AppCssBuilder>,
) {
    let key = widget_key(widget);
    let palette = crate::load_palette_for(output);
    let theme_css = crate::stylesheet_resolved(&crate::Typography::active(), &palette);
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

        let watches = watch_subtree(widget);
        map.insert(
            key,
            WidgetBind {
                output: output.to_string(),
                theme,
                app,
                app_build,
                _watch: watches,
            },
        );
    });

    ensure_destroy_cleanup(widget);
    ensure_themes_watch();
    ensure_palettes_watch();
    ensure_map_reattach(widget);
}

fn ensure_map_reattach(widget: &gtk4::Widget) {
    // `connect_map` once per widget — re-bind already lives in BINDS. Covers the
    // whole-window-remapped case; `watch_subtree` covers lazily-grown subtrees.
    // `MAP_HOOKED` is cleared by `forget_bind` on destroy.
    let key = widget_key(widget);
    let inserted = MAP_HOOKED.with(|s| s.borrow_mut().insert(key));
    if !inserted {
        return;
    }
    widget.connect_map(reattach_bound_tree);
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
    // Record widget -> surface *before* the dedupe check so `forget_bind` can
    // always clear the right `ENTER_HOOKED` key on destroy, even when this call
    // early-returns because the surface was already hooked.
    ENTER_SURFACE.with(|m| m.borrow_mut().insert(widget_key(widget), surf_key));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    }

    // ---- pure logic (no display needed) --------------------------------------

    #[test]
    fn reload_stem_for_event_requires_the_right_extension() {
        assert_eq!(
            reload_stem_for_event(Path::new("/run/user/1/bread/themes/eDP-1.css"), "css")
                .as_deref(),
            Some("eDP-1")
        );
        assert_eq!(
            reload_stem_for_event(Path::new("/x/palettes/HDMI-A-1.json"), "json").as_deref(),
            Some("HDMI-A-1")
        );
        // wrong extension → no reload (this is the finding #6 seam: the palettes
        // watch must accept `.json`, the themes watch only `.css`)
        assert_eq!(
            reload_stem_for_event(Path::new("/x/themes/eDP-1.json"), "css"),
            None
        );
        assert_eq!(
            reload_stem_for_event(Path::new("/x/themes/.eDP-1.css.tmp.42"), "css"),
            None
        );
    }

    #[test]
    fn forget_bind_clears_every_record_including_the_surface_keyed_enter_hook() {
        // Regression for finding #2: `ENTER_HOOKED` is keyed by surface pointer,
        // the rest by widget pointer. A destroyed window used to leave its
        // `ENTER_HOOKED` entry behind forever, so a later window landing on the
        // same surface address never re-themed on a monitor move.
        let wkey = 0x1234_5678_usize;
        let skey = 0x8765_4321_usize;
        ENTER_SURFACE.with(|m| {
            m.borrow_mut().insert(wkey, skey);
        });
        ENTER_HOOKED.with(|s| {
            s.borrow_mut().insert(skey);
        });
        DESTROY_HOOKED.with(|s| {
            s.borrow_mut().insert(wkey);
        });
        AUTO_HOOKED.with(|s| {
            s.borrow_mut().insert(wkey);
        });
        MAP_HOOKED.with(|s| {
            s.borrow_mut().insert(wkey);
        });

        forget_bind(wkey);

        assert!(
            ENTER_HOOKED.with(|s| !s.borrow().contains(&skey)),
            "ENTER_HOOKED entry leaked past destroy"
        );
        assert!(ENTER_SURFACE.with(|m| !m.borrow().contains_key(&wkey)));
        assert!(DESTROY_HOOKED.with(|s| !s.borrow().contains(&wkey)));
        assert!(AUTO_HOOKED.with(|s| !s.borrow().contains(&wkey)));
        assert!(MAP_HOOKED.with(|s| !s.borrow().contains(&wkey)));
    }

    #[test]
    fn dir_reload_watches_arm_and_create_their_directories() {
        // Finding #5: arming must actually succeed in a writable runtime dir,
        // and `arm_or_retry` only falls back to its bounded timer when it
        // genuinely can't.
        let _lock = crate::output::XDG_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("bt-gtk-watch-{}-{}", std::process::id(), nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var("XDG_RUNTIME_DIR").ok();
        std::env::set_var("XDG_RUNTIME_DIR", &dir);

        let themes = try_arm_themes_watch();
        let palettes = try_arm_palettes_watch();

        let themes_dir_made = crate::themes_dir().is_dir();
        let palettes_dir_made = crate::palettes_dir().is_dir();

        match old {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
        let _ = std::fs::remove_dir_all(&dir);

        assert!(themes_dir_made && palettes_dir_made, "watch arming must mkdir -p its dir");
        assert!(themes.is_some(), "themes watch failed to arm in a writable dir");
        assert!(palettes.is_some(), "palettes watch failed to arm in a writable dir");
    }

    // ---- widget lifecycle --------------------------------------------------
    //
    // These need a real GDK display *and* call `gtk4::init()`, which acquires
    // the thread-default glib main context — running them alongside the rest of
    // the suite (where `shell::hotreload` tests pump that context to receive
    // `FileMonitor` events) deadlocks those. So they're `#[ignore]`d by default
    // and the `--features gtk` CI/pre-push command skips them; run them on
    // their own:
    //
    //   cargo test -p bread-theme --features gtk -- --ignored --test-threads=1
    //
    // They still compile with every build, so they can't silently rot.

    // One test only: `gtk4::init()` binds GTK to the calling thread for the
    // life of the process and panics ("two different threads") if any *other*
    // test thread calls it afterwards — so all the widget-level assertions
    // share a single init here.
    #[test]
    #[ignore = "needs a GDK display + exclusive glib main context; run alone (see module note)"]
    fn gtk_widget_lifecycle_and_deep_subtree_reattach() {
        if gtk4::init().is_err() {
            return; // headless
        }

        // Finding #3a: watch_subtree hooks every container, not just the root.
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let mid = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let leaf = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root.append(&mid);
        mid.append(&leaf);
        let watches = watch_subtree(root.upcast_ref());
        assert!(
            watches.borrow().len() >= 3,
            "expected a child-model per container (root/mid/leaf), got {}",
            watches.borrow().len()
        );
        drop(watches);

        // Finding #3b: a grandchild appended *after* the bind must trigger the
        // recursive items-changed hook on `mid` and get re-walked.
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let mid = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root.append(&mid);
        bind_window(&root, "eDP-1");
        let key = widget_key(root.upcast_ref());
        assert!(BINDS.with(|b| b.borrow().contains_key(&key)));

        let before = BINDS.with(|b| b.borrow().get(&key).unwrap()._watch.borrow().len());
        mid.append(&gtk4::Label::new(Some("added late")));
        let after = BINDS.with(|b| b.borrow().get(&key).unwrap()._watch.borrow().len());
        assert!(
            after > before,
            "a widget added two levels below the bound root never triggered re-attach ({before} -> {after})"
        );

        // Finding #2 sibling: forget_bind drops the record.
        forget_bind(key);
        assert!(BINDS.with(|b| !b.borrow().contains_key(&key)));
    }
}
