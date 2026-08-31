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
use crate::matching::{fuzzy_matches, fuzzy_score, split_sections};

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

/// A non-selectable, non-activatable "Recent"/"Apps" label row (plan phase
/// 6c, `[launcher].sections`) — deliberately carries no `"entry"` row data,
/// which is exactly what [`row_entry`] (and everything downstream of it:
/// `set_query`'s filter, `select_next`/`select_prev`'s traversal) already
/// uses to tell a header apart from a real app row.
fn build_header_row(label: &str, idx: u32) -> ListBoxRow {
    let row = ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    row.add_css_class("bread-drawer-section-header");
    let lbl = Label::new(Some(label));
    lbl.add_css_class("section-header-label");
    lbl.set_xalign(0.0);
    row.set_child(Some(&lbl));
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
    ///
    /// `sections` (`[launcher].sections`, plan phase 6c): when true, the
    /// idle (empty-query) view groups `entries` into "Recent"/"Apps"
    /// [`build_header_row`]s via [`split_sections`] instead of one flat
    /// list. Sections disappear the moment a query is typed — `set_query`
    /// falls back to the same flat fuzzy-ranked list either way — so this
    /// only changes the initial build order and the header rows' presence,
    /// never the (unchanged) search behaviour. `false` reproduces the
    /// exact pre-phase-6c flat list breadbox's own overlay still uses.
    pub fn new(
        entries: &[DesktopEntry],
        icon_px: i32,
        history: Rc<RefCell<LaunchHistory>>,
        sections: bool,
    ) -> Self {
        let list = ListBox::new();
        list.set_selection_mode(SelectionMode::Browse);

        let mut idx = 0u32;
        if sections {
            let (recent, apps) = split_sections(entries.to_vec(), &history.borrow());
            if !recent.is_empty() {
                list.append(&build_header_row("Recent", idx));
                idx += 1;
                for entry in &recent {
                    list.append(&build_row(entry, idx, icon_px));
                    idx += 1;
                }
            }
            if !apps.is_empty() {
                list.append(&build_header_row("Apps", idx));
                idx += 1;
                for entry in &apps {
                    list.append(&build_row(entry, idx, icon_px));
                    idx += 1;
                }
            }
        } else {
            for entry in entries {
                list.append(&build_row(entry, idx, icon_px));
                idx += 1;
            }
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
                // A header row carries no "entry" data — sort it after any
                // real row rather than treating the comparison as `Equal`,
                // though `set_query` also hides every header outright once
                // a query is non-empty, so this only matters for the
                // underlying (invisible) list order, never what's shown.
                match (row_entry(row_a), row_entry(row_b)) {
                    (Some(ea), Some(eb)) => {
                        let sa = fuzzy_score(&query, &ea);
                        let sb = fuzzy_score(&query, &eb);
                        let history = history.borrow();
                        let ca = history.count(&ea.name);
                        let cb = history.count(&eb.name);
                        sa.cmp(&sb)
                            .then(cb.cmp(&ca))
                            .then(ea.name.to_lowercase().cmp(&eb.name.to_lowercase()))
                            .into()
                    }
                    (None, Some(_)) => std::cmp::Ordering::Greater.into(),
                    (Some(_), None) => std::cmp::Ordering::Less.into(),
                    (None, None) => std::cmp::Ordering::Equal.into(),
                }
            });
        }

        let first_real = (0i32..)
            .map_while(|i| list.row_at_index(i))
            .find(|r| row_entry(r).is_some());
        if let Some(first) = first_real {
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
    /// re-sorts by `query`, then selects the first visible row. A header
    /// row (see [`build_header_row`]) only ever shows in the idle
    /// (empty-query) browse view — it has no name/`wm_class`/`exec` of its
    /// own to filter against.
    pub fn set_query(&self, query: &str) {
        *self.query.borrow_mut() = query.to_string();
        let mut i = 0i32;
        while let Some(row) = self.list.row_at_index(i) {
            let vis = match row_entry(&row) {
                Some(e) => {
                    fuzzy_matches(query, &e.name)
                        || e.wm_class.as_deref().is_some_and(|w| fuzzy_matches(query, w))
                        || fuzzy_matches(query, &e.exec)
                }
                None => query.is_empty(),
            };
            row.set_visible(vis);
            i += 1;
        }
        self.list.invalidate_sort();
        let first_vis = (0i32..)
            .map_while(|j| self.list.row_at_index(j))
            .find(|r| r.is_visible() && row_entry(r).is_some());
        self.list.select_row(first_vis.as_ref());
    }

    pub fn selected_entry(&self) -> Option<DesktopEntry> {
        self.list.selected_row().and_then(|r| row_entry(&r))
    }

    /// Moves the selection to the next visible row, if any. Skips header
    /// rows even though they may be visible (the idle browse view) —
    /// `set_selectable(false)` alone doesn't stop a programmatic
    /// `select_row` call from landing on one.
    pub fn select_next(&self) {
        let cur = self.list.selected_row().map(|r| r.index()).unwrap_or(-1);
        let mut i = cur + 1;
        loop {
            match self.list.row_at_index(i) {
                Some(r) if r.is_visible() && row_entry(&r).is_some() => {
                    self.list.select_row(Some(&r));
                    break;
                }
                Some(_) => i += 1,
                None => break,
            }
        }
    }

    /// Moves the selection to the previous visible row, if any. See
    /// [`select_next`](Self::select_next) on skipping header rows.
    pub fn select_prev(&self) {
        let cur = self.list.selected_row().map(|r| r.index()).unwrap_or(0);
        let mut i = cur - 1;
        loop {
            if i < 0 {
                break;
            }
            match self.list.row_at_index(i) {
                Some(r) if r.is_visible() && row_entry(&r).is_some() => {
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
