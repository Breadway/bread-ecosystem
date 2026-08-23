//! `watch()` — only compiled under the `gtk` feature, since
//! `gio::FileMonitor` is a gtk4 dependency and the rest of `shell` is
//! deliberately gtk-free (`bread`/`breadcrumbs` link this crate without the
//! `gtk` feature at all).

use gtk4::gio;
use gtk4::prelude::*;

/// Fires `f` with a freshly-[`super::load`]ed [`super::ShellTheme`] whenever
/// the active theme's own directory changes on disk. Keep the returned
/// monitor alive — dropping it disarms the watch.
///
/// Watches the *directory*, not the `theme.toml` file itself, for the same
/// reason `bread_theme::gtk::watch_theme_file` does (see that function's doc
/// comment): an editor or `bread-theme` doing an atomic write-tmp-then-
/// rename replaces the inode, and a monitor on the file itself dies after
/// the first replace (inotify reports `DELETE_SELF` and never re-arms).
pub fn watch<F: Fn(super::ShellTheme) + 'static>(f: F) -> gio::FileMonitor {
    let id = super::active_theme_id();
    let dir = super::user_theme_path(&id)
        .parent()
        .expect("user_theme_path always has a parent")
        .to_path_buf();
    let _ = std::fs::create_dir_all(&dir);
    let monitor = gio::File::for_path(&dir)
        .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        .expect("failed to create a file monitor for the shell theme directory");
    monitor.connect_changed(move |_, _file, _other, _event| {
        f(super::load());
    });
    monitor
}
