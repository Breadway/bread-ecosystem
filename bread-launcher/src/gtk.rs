//! GTK4 results-list widget: the "row-building half" of what used to be
//! breadbox's `run_ui` (`THEME_SYSTEM_PLAN.md` §3) — desktop-entry rows,
//! fuzzy filtering, match/history sorting, and keyboard-style selection
//! movement, packaged as [`ResultsList`] so any host window can embed it.
//! breadbox wraps it in a full-screen overlay window today; breadbar's
//! embedded capsule (a later phase) puts the same widget in its drawer slot.

use std::{cell::RefCell, path::Path, rc::Rc};

use gtk4::{
    gdk, gio,
    pango::EllipsizeMode,
    prelude::*,
    Align, Box as GBox, Image, Label, ListBox, ListBoxRow, Orientation, PolicyType,
    ScrolledWindow, SelectionMode,
};

use crate::desktop::DesktopEntry;
use crate::history::LaunchHistory;
use crate::matching::{fuzzy_matches, fuzzy_score};

fn make_icon(icon_name: &str, icon_path: Option<&Path>, icon_px: i32) -> Image {
    // Try loading from resolved cached path via gio::File
    if let Some(path) = icon_path {
        let gio_file = gio::File::for_path(path);
        if let Ok(texture) = gdk::Texture::from_file(&gio_file) {
            let img = Image::new();
            img.set_paintable(Some(&texture));
            img.set_pixel_size(icon_px);
            return img;
        }
    }
    // Fall back to GTK icon theme lookup by name
    let name = if icon_name.is_empty() {
        "application-x-executable"
    } else {
        icon_name
    };
    let img = Image::from_icon_name(name);
    img.set_pixel_size(icon_px);
    img
}

fn build_row(entry: &DesktopEntry, idx: u32, icon_px: i32) -> ListBoxRow {
    let row = ListBoxRow::new();
    let hbox = GBox::new(Orientation::Horizontal, 0);
    hbox.set_margin_start(6);
    hbox.set_margin_end(6);
    hbox.set_valign(Align::Center);

    let icon = make_icon(&entry.icon_name, entry.icon_path.as_deref(), icon_px);
    hbox.append(&icon);

    let name_lbl = Label::new(Some(&entry.name));
    name_lbl.add_css_class("app-name");
    name_lbl.set_xalign(0.0);
    name_lbl.set_hexpand(true);
    name_lbl.set_ellipsize(EllipsizeMode::End);
    hbox.append(&name_lbl);

    if let Some(ref wm) = entry.wm_class {
        let wm_lbl = Label::new(Some(wm));
        wm_lbl.add_css_class("app-muted");
        wm_lbl.set_xalign(1.0);
        hbox.append(&wm_lbl);
    }

    row.set_child(Some(&hbox));
    unsafe { row.set_data("entry", entry.clone()) };
    unsafe { row.set_data("initial_order", idx) };
    row
}

/// Reads the [`DesktopEntry`] a row was built from — e.g. from a
/// `ListBox::connect_row_activated` handler, which hands back a row
/// reference rather than going through [`ResultsList::selected_entry`].
pub fn row_entry(row: &ListBoxRow) -> Option<DesktopEntry> {
    unsafe { row.data::<DesktopEntry>("entry").map(|p| p.as_ref().clone()) }
}

/// A scrollable, filterable, rankable list of desktop-entry rows — the
/// widget breadbox's overlay wraps today and breadbar's capsule will embed
/// next (`THEME_SYSTEM_PLAN.md` §7). A host drives it through
/// [`set_query`](Self::set_query) (wire to a search entry's `changed`
/// signal), [`select_next`](Self::select_next)/[`select_prev`](Self::select_prev)
/// (wire to arrow keys), and reads the current pick via
/// [`selected_entry`](Self::selected_entry) — `list`/`scroller` are exposed
/// directly for anything else a host needs (e.g. `connect_row_activated`
/// for click-to-launch, or placing `scroller` in a slot).
#[derive(Clone)]
pub struct ResultsList {
    pub scroller: ScrolledWindow,
    pub list: ListBox,
    query: Rc<RefCell<String>>,
    history: Rc<RefCell<LaunchHistory>>,
}

impl ResultsList {
    /// Builds one row per entry (in `entries`' given order — that order is
    /// also the fallback sort when the query is empty) and wires up sorting
    /// against `history`'s launch counts.
    pub fn new(entries: &[DesktopEntry], icon_px: i32, history: Rc<RefCell<LaunchHistory>>) -> Self {
        let list = ListBox::new();
        list.set_selection_mode(SelectionMode::Browse);

        for (idx, entry) in entries.iter().enumerate() {
            list.append(&build_row(entry, idx as u32, icon_px));
        }

        let query: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        {
            let query = Rc::clone(&query);
            let history = Rc::clone(&history);
            list.set_sort_func(move |row_a, row_b| {
                let query = query.borrow();
                if query.is_empty() {
                    let oa = unsafe {
                        row_a.data::<u32>("initial_order").map_or(u32::MAX, |p| *p.as_ref())
                    };
                    let ob = unsafe {
                        row_b.data::<u32>("initial_order").map_or(u32::MAX, |p| *p.as_ref())
                    };
                    return oa.cmp(&ob).into();
                }
                let (Some(ea), Some(eb)) = (row_entry(row_a), row_entry(row_b)) else {
                    return std::cmp::Ordering::Equal.into();
                };
                let sa = fuzzy_score(&query, &ea);
                let sb = fuzzy_score(&query, &eb);
                let history = history.borrow();
                let ca = history.count(&ea.name);
                let cb = history.count(&eb.name);
                sa.cmp(&sb)
                    .then(cb.cmp(&ca))
                    .then(ea.name.to_lowercase().cmp(&eb.name.to_lowercase()))
                    .into()
            });
        }

        if let Some(first) = list.row_at_index(0) {
            list.select_row(Some(&first));
        }

        let scroller = ScrolledWindow::new();
        scroller.set_policy(PolicyType::Never, PolicyType::Automatic);
        scroller.set_max_content_height(480);
        scroller.set_propagate_natural_height(true);
        scroller.set_child(Some(&list));

        ResultsList { scroller, list, query, history }
    }

    /// Re-filters (fuzzy match against name, `wm_class`, and `exec`) and
    /// re-sorts by `query`, then selects the first visible row.
    pub fn set_query(&self, query: &str) {
        *self.query.borrow_mut() = query.to_string();
        let mut i = 0i32;
        while let Some(row) = self.list.row_at_index(i) {
            let vis = row_entry(&row)
                .map(|e| {
                    fuzzy_matches(query, &e.name)
                        || e.wm_class.as_deref().is_some_and(|w| fuzzy_matches(query, w))
                        || fuzzy_matches(query, &e.exec)
                })
                .unwrap_or(false);
            row.set_visible(vis);
            i += 1;
        }
        self.list.invalidate_sort();
        let first_vis = (0i32..).find_map(|j| self.list.row_at_index(j).filter(|r| r.is_visible()));
        self.list.select_row(first_vis.as_ref());
    }

    pub fn selected_entry(&self) -> Option<DesktopEntry> {
        self.list.selected_row().and_then(|r| row_entry(&r))
    }

    /// Moves the selection to the next visible row, if any.
    pub fn select_next(&self) {
        let cur = self.list.selected_row().map(|r| r.index()).unwrap_or(-1);
        let mut i = cur + 1;
        loop {
            match self.list.row_at_index(i) {
                Some(r) if r.is_visible() => {
                    self.list.select_row(Some(&r));
                    break;
                }
                Some(_) => i += 1,
                None => break,
            }
        }
    }

    /// Moves the selection to the previous visible row, if any.
    pub fn select_prev(&self) {
        let cur = self.list.selected_row().map(|r| r.index()).unwrap_or(0);
        let mut i = cur - 1;
        loop {
            if i < 0 {
                break;
            }
            match self.list.row_at_index(i) {
                Some(r) if r.is_visible() => {
                    self.list.select_row(Some(&r));
                    break;
                }
                Some(_) => i -= 1,
                None => break,
            }
        }
    }

    /// Records `entry` as launched in the shared history and persists it.
    /// Call before actually launching (matching breadbox's original
    /// increment-then-launch ordering) — history and launching are separate
    /// concerns, so this doesn't call [`crate::do_launch`] itself.
    pub fn record_launch(&self, entry: &DesktopEntry) {
        self.history.borrow_mut().increment(&entry.name);
        self.history.borrow().save();
    }
}
