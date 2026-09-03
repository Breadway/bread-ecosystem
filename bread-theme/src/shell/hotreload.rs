//! `watch()` — only compiled under the `gtk` feature, since
//! `gio::FileMonitor` is a gtk4 dependency and the rest of `shell` is
//! deliberately gtk-free (`bread`/`breadcrumbs` link this crate without the
//! `gtk` feature at all).

use gtk4::gio;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// The handle [`watch`] returns. Keep it alive — dropping it disarms both
/// the config-file watch and whichever theme directory is currently armed;
/// there is nothing else to call on it.
///
/// Two independent [`gio::FileMonitor`]s live behind this, not one:
/// - a fixed watch on `~/.config/bread/` (or `$XDG_CONFIG_HOME/bread/`)
///   for `shell.toml` itself, so a change to the *active* theme id is
///   noticed at all;
/// - a swappable watch on whichever theme directory is currently active,
///   re-armed onto the new directory whenever the config watch observes
///   `active` changing (see [`watch`]'s doc comment for what this does and
///   does not cover).
pub struct ThemeWatch {
    _config_monitor: Option<gio::FileMonitor>,
    // `Rc<RefCell<..>>` (rather than a plain field) because the config
    // watch's own callback needs to replace this monitor in place when the
    // active theme changes, while this struct is what keeps it alive for
    // the caller.
    _theme_monitor: Rc<RefCell<Option<gio::FileMonitor>>>,
}

/// Best-effort: `monitor_directory` on `dir` can fail (e.g.
/// `fs.inotify.max_user_watches` exhausted) — that must never be fatal, per
/// this crate's "the shell must never fail to start because a theme file is
/// malformed" stance (`shell` module doc). Logs once per call site and
/// returns `None` rather than panicking; the caller simply runs without a
/// live watch for whatever this was meant to cover, same as
/// `bread_theme::gtk::watch_theme_file`'s existing `.ok()?` stance for the
/// shared-stylesheet watch.
fn monitor_dir(dir: &std::path::Path, what: &str) -> Option<gio::FileMonitor> {
    let _ = std::fs::create_dir_all(dir);
    match gio::File::for_path(dir)
        .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
    {
        Ok(m) => Some(m),
        Err(err) => {
            tracing::warn!(
                "bread-theme: could not create a file monitor for {what} ({}); \
                 continuing without live reload for it",
                err
            );
            None
        }
    }
}

fn theme_dir(id: &str) -> PathBuf {
    super::user_theme_path(id)
        .parent()
        .expect("user_theme_path always has a parent")
        .to_path_buf()
}

/// Quiet period after the last filesystem event before the theme is
/// re-`load()`ed. An editor / `bread-theme` save is 2–4 events (truncate,
/// write, close, or write-tmp + rename); a non-atomic `cat >` momentarily
/// leaves the file truncated. Coalescing them into one `load()` after the
/// dust settles avoids both a burst of redundant reloads and a transient
/// parse-failure fall-back to the builtin.
const DEBOUNCE_MS: u32 = 120;

/// Wrap `f` so repeated calls within [`DEBOUNCE_MS`] collapse into a single
/// `f(super::load())` fired once the calls stop.
fn debounced(f: Rc<dyn Fn(super::ShellTheme)>) -> Rc<dyn Fn()> {
    let pending: Rc<RefCell<Option<gtk4::glib::SourceId>>> = Rc::new(RefCell::new(None));
    Rc::new(move || {
        if let Some(id) = pending.borrow_mut().take() {
            id.remove();
        }
        let f = f.clone();
        let pending_inner = pending.clone();
        let id = gtk4::glib::timeout_add_local_once(
            std::time::Duration::from_millis(DEBOUNCE_MS as u64),
            move || {
                *pending_inner.borrow_mut() = None;
                f(super::load());
            },
        );
        *pending.borrow_mut() = Some(id);
    })
}

/// (Re-)arms `theme_monitor` on `id`'s own directory, replacing whatever was
/// armed before (dropping the old [`gio::FileMonitor`] disarms it).
fn arm_theme_watch(
    theme_monitor: &Rc<RefCell<Option<gio::FileMonitor>>>,
    id: &str,
    trigger: &Rc<dyn Fn()>,
) {
    let dir = theme_dir(id);
    let monitor = monitor_dir(
        &dir,
        &format!("theme '{id}'s directory ({})", dir.display()),
    );
    if let Some(m) = &monitor {
        let trigger = trigger.clone();
        m.connect_changed(move |_, _file, _other, _event| trigger());
    }
    *theme_monitor.borrow_mut() = monitor;
}

/// Fires `f` with a freshly-[`super::load`]ed [`super::ShellTheme`] whenever
/// the active theme's own directory changes on disk, AND re-arms itself onto
/// a new theme's directory when `~/.config/bread/shell.toml`'s `active` key
/// changes — so switching themes while a host is already running (e.g.
/// bos-settings rewriting `active =`) picks up live edits to the *new*
/// theme without a restart, not just edits to whichever theme happened to
/// be active when `watch()` was first called.
///
/// Watches directories, not the files themselves, for the same reason
/// `bread_theme::gtk::watch_theme_file` does (see that function's doc
/// comment): an editor or `bread-theme` doing an atomic write-tmp-then-
/// rename replaces the inode, and a monitor on the file itself dies after
/// the first replace (inotify reports `DELETE_SELF` and never re-arms).
///
/// # What this does NOT cover
///
/// `$BREAD_SHELL_THEME` is a **single-process override for testing**, not
/// the primary theme selector (`shell.toml`'s `active` key is — see the
/// `shell` module doc's "Discovery" section) — precisely because an env var
/// set for one process is invisible to any other, it cannot coordinate two
/// already-running hosts (breadbar/breadbox) onto the same theme, and this
/// watch cannot observe it changing either: it's read once per call to
/// [`super::active_theme_id`] (i.e. once per `load()`/re-arm here), and
/// there is no filesystem event to watch for a process's own environment
/// changing out from under it. A host that wants to pick up a new
/// `$BREAD_SHELL_THEME` value has to restart, same as before this fix.
pub fn watch<F: Fn(super::ShellTheme) + 'static>(f: F) -> ThemeWatch {
    let f: Rc<dyn Fn(super::ShellTheme)> = Rc::new(f);
    let trigger = debounced(f.clone());

    let watched_id = Rc::new(RefCell::new(super::active_theme_id()));
    let theme_monitor: Rc<RefCell<Option<gio::FileMonitor>>> = Rc::new(RefCell::new(None));
    arm_theme_watch(&theme_monitor, &watched_id.borrow(), &trigger);

    let config_dir = super::config_home().join("bread");
    let config_monitor = monitor_dir(
        &config_dir,
        &format!("the shell config directory ({})", config_dir.display()),
    );
    if let Some(cm) = &config_monitor {
        let theme_monitor = theme_monitor.clone();
        let watched_id = watched_id.clone();
        let f = f.clone();
        let trigger = trigger.clone();
        cm.connect_changed(move |_, file, other, _event| {
            // Only `shell.toml` changing can move `active` — ignore any
            // other file (e.g. a theme directory that happens to also live
            // under this same parent) so this doesn't re-check on every
            // unrelated write in ~/.config/bread/.
            let is_shell_toml = |f: &gio::File| {
                f.basename().and_then(|b| b.to_str().map(str::to_string))
                    == Some("shell.toml".to_string())
            };
            if !is_shell_toml(file) && !other.is_some_and(is_shell_toml) {
                return;
            }
            let new_id = super::active_theme_id();
            if new_id == *watched_id.borrow() {
                // shell.toml changed but `active` didn't (or moved to the
                // same value) — the already-armed theme watch still covers
                // whatever changed.
                return;
            }
            // An `active` switch is a deliberate action (bos-settings, an
            // edit) — re-arm and load now, not after the debounce.
            *watched_id.borrow_mut() = new_id.clone();
            arm_theme_watch(&theme_monitor, &new_id, &trigger);
            f(super::load());
        });
    }

    ThemeWatch {
        _config_monitor: config_monitor,
        _theme_monitor: theme_monitor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Isolated `$XDG_CONFIG_HOME`, mirroring `shell::tests::isolated_xdg`
    /// (can't reuse it directly — it's private to the parent module's own
    /// `tests` submodule — but shares the same lock so the two test files
    /// never race the same env vars).
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        dir: PathBuf,
        old_xdg: Option<String>,
        old_theme_var: Option<String>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.old_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match &self.old_theme_var {
                Some(v) => std::env::set_var("BREAD_SHELL_THEME", v),
                None => std::env::remove_var("BREAD_SHELL_THEME"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn isolated_xdg() -> EnvGuard {
        let lock = crate::test_support::XDG_CONFIG_HOME_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "bread-theme-hotreload-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let old_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let old_theme_var = std::env::var("BREAD_SHELL_THEME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        std::env::remove_var("BREAD_SHELL_THEME");
        EnvGuard {
            _lock: lock,
            dir,
            old_xdg,
            old_theme_var,
        }
    }

    fn write_theme(xdg: &EnvGuard, id: &str, toml_body: &str) {
        let dir = xdg.dir.join("bread/themes").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("theme.toml"), toml_body).unwrap();
    }

    fn write_shell_toml(xdg: &EnvGuard, active: &str) {
        let dir = xdg.dir.join("bread");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shell.toml"), format!("active = \"{active}\"\n")).unwrap();
    }

    /// Pumps the default `glib::MainContext` (where `gio::FileMonitor`
    /// dispatches its `connect_changed` signal) until `done()` returns
    /// true or `timeout` elapses. Real inotify events go through the OS,
    /// so this polls rather than blocking on a single iteration.
    fn pump_until(timeout: std::time::Duration, mut done: impl FnMut() -> bool) {
        let start = std::time::Instant::now();
        let ctx = gtk4::glib::MainContext::default();
        while !done() && start.elapsed() < timeout {
            while ctx.iteration(false) {}
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn watch_arms_a_monitor_for_the_currently_active_theme() {
        let xdg = isolated_xdg();
        write_theme(&xdg, "liquid-motion", "id = \"liquid-motion\"\n");
        let watch = watch(|_| {});
        assert!(
            watch._theme_monitor.borrow().is_some(),
            "watch() should arm a monitor for the active theme's directory"
        );
        assert!(
            watch._config_monitor.is_some(),
            "watch() should arm a monitor for the shell config directory too"
        );
    }

    #[test]
    fn editing_the_active_theme_directory_fires_the_callback() {
        let xdg = isolated_xdg();
        write_theme(&xdg, "custom", "id = \"custom\"\n[tokens]\npad = 1\n");
        write_shell_toml(&xdg, "custom");

        let calls = Rc::new(RefCell::new(0));
        let calls2 = calls.clone();
        let _watch = watch(move |_| *calls2.borrow_mut() += 1);

        write_theme(&xdg, "custom", "id = \"custom\"\n[tokens]\npad = 2\n");
        pump_until(std::time::Duration::from_secs(3), || *calls.borrow() > 0);

        assert!(
            *calls.borrow() > 0,
            "editing the active theme's own directory should fire the watch callback"
        );
    }

    #[test]
    fn switching_the_active_theme_re_arms_onto_the_new_directory() {
        // This is the coordination gap the fix closes: before it, `watch()`
        // resolved the active theme's directory once and pinned a monitor
        // there forever, so a `shell.toml` `active` switch left the OLD
        // theme's edits observed and the NEW theme's edits invisible until
        // a restart.
        let xdg = isolated_xdg();
        write_theme(&xdg, "theme-a", "id = \"theme-a\"\n[tokens]\npad = 1\n");
        write_theme(&xdg, "theme-b", "id = \"theme-b\"\n[tokens]\npad = 1\n");
        write_shell_toml(&xdg, "theme-a");

        let seen_ids = Rc::new(RefCell::new(Vec::<String>::new()));
        let seen_ids2 = seen_ids.clone();
        let _watch = watch(move |theme| seen_ids2.borrow_mut().push(theme.id().to_string()));

        // Switch active to theme-b.
        write_shell_toml(&xdg, "theme-b");
        pump_until(std::time::Duration::from_secs(3), || {
            seen_ids.borrow().iter().any(|id| id == "theme-b")
        });
        assert!(
            seen_ids.borrow().iter().any(|id| id == "theme-b"),
            "switching shell.toml's active id should fire the callback with the new theme: {:?}",
            seen_ids.borrow()
        );

        seen_ids.borrow_mut().clear();

        // Now edit theme-b's own directory — the re-armed watch must see it.
        write_theme(&xdg, "theme-b", "id = \"theme-b\"\n[tokens]\npad = 2\n");
        pump_until(std::time::Duration::from_secs(3), || {
            !seen_ids.borrow().is_empty()
        });
        assert!(
            !seen_ids.borrow().is_empty(),
            "after switching, editing the NEW active theme's directory must fire the callback"
        );
    }

    #[test]
    fn monitor_dir_degrades_gracefully_instead_of_panicking() {
        // A directory that cannot possibly be created (its parent is a
        // plain file, not a directory) — `create_dir_all` fails, and this
        // must not panic either way `monitor_directory` then behaves.
        let xdg = isolated_xdg();
        let blocker = xdg.dir.join("not-a-directory");
        std::fs::write(&blocker, b"x").unwrap();
        let impossible = blocker.join("child").join("grandchild");

        // Must not panic; may or may not return a monitor depending on
        // glib's own behavior for a directory that doesn't exist yet, but
        // this call itself is infallible from the caller's point of view.
        let _ = monitor_dir(&impossible, "test");
    }
}
