use std::{collections::HashMap, path::PathBuf};

use crate::desktop::{load_all_desktop_entries, DesktopEntry};
use crate::history::LaunchHistory;

// ---- Fuzzy matching (query filter) ------------------------------------------

/// Subsequence match used to *filter* rows as the user types: every char of
/// `pattern`, in order, must appear somewhere in `text` (case-insensitive).
/// Looser than [`fuzzy_score`], which ranks the rows that pass this filter.
pub fn fuzzy_matches(pattern: &str, text: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    let mut chars = text.chars();
    for pc in pattern.chars() {
        let pl = pc.to_lowercase().next().unwrap_or(pc);
        if !chars
            .by_ref()
            .any(|tc| tc.to_lowercase().next().unwrap_or(tc) == pl)
        {
            return false;
        }
    }
    true
}

/// Ranks how well `query` matches `entry` — lower is better. Exact match (by
/// name or `wm_class`) sorts first, then name-prefix, then name-contains,
/// then `wm_class`-prefix/contains, then everything else that still passed
/// [`fuzzy_matches`] (a subsequence match with no stronger relationship).
pub fn fuzzy_score(query: &str, entry: &DesktopEntry) -> u32 {
    let q = query.to_lowercase();
    let name = entry.name.to_lowercase();
    let wm = entry.wm_class.as_deref().unwrap_or("").to_lowercase();
    if name == q || wm == q {
        return 0;
    }
    if name.starts_with(&q) {
        return 1;
    }
    if name.contains(&q) {
        return 2;
    }
    if wm.starts_with(&q) || wm.contains(&q) {
        return 3;
    }
    4 // subsequence match
}

// ---- Priority ranking (empty-query ordering) --------------------------------

/// Whole-word / exact match of `term` within `field` (both lowercase). Avoids
/// "code" matching "vscodium" while still matching "Code", "code-oss", and
/// "Visual Studio Code".
pub fn matches_term(field: &str, term: &str) -> bool {
    if term.is_empty() || field.is_empty() {
        return false;
    }
    if field == term {
        return true;
    }
    let bytes = field.as_bytes();
    let tlen = term.len();
    let mut start = 0;
    while let Some(pos) = field[start..].find(term) {
        let i = start + pos;
        let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let after = i + tlen;
        let after_ok = after >= bytes.len() || !bytes[after].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        // Advance past the WHOLE match, not one byte past its start. Both `i`
        // (a match start) and `i + tlen` (its end) are guaranteed char
        // boundaries; `i + 1` is not, so a multi-byte term that failed the
        // word-boundary check left `start` inside a character and the next
        // `field[start..]` slice panicked outright. `matches_term("café", "é")`
        // reproduced it: "byte index 4 is not a char boundary". Reached via
        // priority_rank over real .desktop `Name=` values, so any non-ASCII
        // app name could crash the launcher's sort.
        start = i + tlen;
        if start >= field.len() {
            break;
        }
    }
    false
}

/// Position of `entry` in the (already-lowercased) `priority` list, matched
/// against either its name or `wm_class`. `None` if `entry` isn't named
/// there at all.
pub fn priority_rank(entry: &DesktopEntry, priority_lower: &[String]) -> Option<usize> {
    let name_l = entry.name.to_lowercase();
    let wm_l = entry.wm_class.as_deref().unwrap_or("").to_lowercase();
    priority_lower
        .iter()
        .position(|p| matches_term(&name_l, p) || matches_term(&wm_l, p))
}

/// Loads every known desktop entry, resolves each one's icon path from
/// `manifest`, and sorts them: entries named in `priority` come first (in
/// that order), then everything else by most-launched (via `history`), then
/// alphabetically.
pub fn load_sorted_entries(
    manifest: &HashMap<String, PathBuf>,
    priority: &[String],
    history: &LaunchHistory,
) -> Vec<DesktopEntry> {
    let mut entries = load_all_desktop_entries();

    // Populate icon_path from manifest
    for entry in &mut entries {
        if let Some(path) = manifest.get(&entry.icon_name) {
            if path.exists() {
                entry.icon_path = Some(path.clone());
            }
        }
    }

    let priority_lower: Vec<String> = priority.iter().map(|s| s.to_lowercase()).collect();

    entries.sort_by(|a, b| {
        let ai = priority_rank(a, &priority_lower);
        let bi = priority_rank(b, &priority_lower);
        match (ai, bi) {
            (Some(i), Some(j)) => i.cmp(&j),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => {
                // Most-launched first, then alphabetical
                history
                    .count(&b.name)
                    .cmp(&history.count(&a.name))
                    .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            }
        }
    });

    entries
}

/// The demo's own cap (`BOS.pushRecent`'s `.slice(0, 4)`) on how many
/// entries the "Recent" section shows.
pub const MAX_RECENT: usize = 4;

/// Splits `entries` (already ordered by [`load_sorted_entries`]) into a
/// "recent" section — the entries `history` has any launch count for, most-
/// launched first, capped at [`MAX_RECENT`] — and an "apps" section: every
/// other entry, in its existing relative order. No new tracking beyond
/// `LaunchHistory`'s existing counts (THEME_SYSTEM_PLAN.md phase 6c task
/// notes: "`LaunchHistory` already tracks counts for a recents list").
///
/// Only meaningful when `entries` has no priority-ranked prefix (breadbar's
/// capsule calls [`load_sorted_entries`] with an empty `priority` list) —
/// with a non-empty priority list, priority-ranked entries still sort first
/// and would be treated as "apps" here even if launched often, since this
/// function has no way to tell "sorted first because launched a lot" from
/// "sorted first because priority-ranked" apart from the count itself.
pub fn split_sections(
    entries: Vec<DesktopEntry>,
    history: &LaunchHistory,
) -> (Vec<DesktopEntry>, Vec<DesktopEntry>) {
    let mut recent = Vec::new();
    let mut apps = Vec::new();
    for entry in entries {
        if recent.len() < MAX_RECENT && history.count(&entry.name) > 0 {
            recent.push(entry);
        } else {
            apps.push(entry);
        }
    }
    (recent, apps)
}

#[cfg(test)]
mod tests {
    #[test]
    fn multibyte_term_that_fails_word_boundary_does_not_panic() {
        // "é" occurs in "café" but is preceded by an alphanumeric, so the
        // whole-word check fails and the scan must continue — landing mid
        // character before the fix.
        assert!(!super::matches_term("café", "é"));
        assert!(!super::matches_term("naïve café", "ï"));
    }

    #[test]
    fn multibyte_whole_word_still_matches() {
        assert!(super::matches_term("café bar", "café"));
        assert!(super::matches_term("día", "día"));
    }

    use super::*;

    fn entry(name: &str, wm_class: Option<&str>) -> DesktopEntry {
        DesktopEntry {
            id: format!("{name}.desktop"),
            name: name.to_string(),
            exec: "true".to_string(),
            icon_name: String::new(),
            icon_path: None,
            categories: Vec::new(),
            wm_class: wm_class.map(|s| s.to_string()),
            terminal: false,
        }
    }

    // ---- fuzzy_matches -------------------------------------------------

    #[test]
    fn fuzzy_matches_empty_pattern_matches_anything() {
        assert!(fuzzy_matches("", "Firefox"));
        assert!(fuzzy_matches("", ""));
    }

    #[test]
    fn fuzzy_matches_in_order_subsequence() {
        assert!(fuzzy_matches("ffx", "Firefox"));
        assert!(fuzzy_matches("frfx", "Firefox"));
    }

    #[test]
    fn fuzzy_matches_is_case_insensitive() {
        assert!(fuzzy_matches("FIREFOX", "firefox"));
        assert!(fuzzy_matches("firefox", "FireFox"));
    }

    #[test]
    fn fuzzy_matches_rejects_out_of_order() {
        assert!(!fuzzy_matches("xfr", "Firefox"));
    }

    #[test]
    fn fuzzy_matches_rejects_missing_chars() {
        assert!(!fuzzy_matches("firefoxx", "Firefox"));
    }

    // ---- fuzzy_score -----------------------------------------------------

    #[test]
    fn fuzzy_score_exact_name_match_is_best() {
        let e = entry("Firefox", None);
        assert_eq!(fuzzy_score("firefox", &e), 0);
    }

    #[test]
    fn fuzzy_score_exact_wm_class_match_is_best() {
        let e = entry("Firefox Web Browser", Some("firefox"));
        assert_eq!(fuzzy_score("firefox", &e), 0);
    }

    #[test]
    fn fuzzy_score_name_prefix_beats_name_contains() {
        let prefix = entry("Firefox", None);
        let contains = entry("GNU IceCat (Firefox fork)", None);
        assert_eq!(fuzzy_score("fire", &prefix), 1);
        assert_eq!(fuzzy_score("fire", &contains), 2);
        assert!(fuzzy_score("fire", &prefix) < fuzzy_score("fire", &contains));
    }

    #[test]
    fn fuzzy_score_wm_class_beats_pure_subsequence() {
        let wm_hit = entry("Web Browser", Some("firefox"));
        let subseq_only = entry("Fine Iris Reflex Editor for XML", None);
        assert_eq!(fuzzy_score("fire", &wm_hit), 3);
        assert_eq!(fuzzy_score("fire", &subseq_only), 4);
    }

    // ---- matches_term ------------------------------------------------------

    #[test]
    fn matches_term_exact_field_matches() {
        assert!(matches_term("code", "code"));
    }

    #[test]
    fn matches_term_whole_word_within_longer_field() {
        assert!(matches_term("visual studio code", "code"));
    }

    #[test]
    fn matches_term_rejects_substring_of_a_larger_word() {
        // "code" must not match inside "vscodium" — this is the whole
        // reason matches_term exists instead of a plain `contains`.
        assert!(!matches_term("vscodium", "code"));
    }

    #[test]
    fn matches_term_matches_hyphenated_variant() {
        assert!(matches_term("code-oss", "code"));
    }

    #[test]
    fn matches_term_empty_term_or_field_never_matches() {
        assert!(!matches_term("code", ""));
        assert!(!matches_term("", "code"));
    }

    // ---- priority_rank -----------------------------------------------------

    #[test]
    fn priority_rank_matches_by_name() {
        let e = entry("Firefox", None);
        let priority = vec!["firefox".to_string(), "code".to_string()];
        assert_eq!(priority_rank(&e, &priority), Some(0));
    }

    #[test]
    fn priority_rank_matches_by_wm_class() {
        let e = entry("Web Browser", Some("firefox"));
        let priority = vec!["code".to_string(), "firefox".to_string()];
        assert_eq!(priority_rank(&e, &priority), Some(1));
    }

    #[test]
    fn priority_rank_none_when_unlisted() {
        let e = entry("Nautilus", None);
        let priority = vec!["firefox".to_string()];
        assert_eq!(priority_rank(&e, &priority), None);
    }

    #[test]
    fn priority_rank_does_not_match_substring_of_a_word() {
        let e = entry("VSCodium", None);
        let priority = vec!["code".to_string()];
        assert_eq!(priority_rank(&e, &priority), None);
    }

    // ---- split_sections --------------------------------------------------

    #[test]
    fn split_sections_no_history_is_all_apps() {
        let entries = vec![entry("Firefox", None), entry("GoLand", None)];
        let history = LaunchHistory::from_counts(HashMap::new());
        let (recent, apps) = split_sections(entries, &history);
        assert!(recent.is_empty());
        assert_eq!(apps.len(), 2);
    }

    #[test]
    fn split_sections_launched_entries_go_to_recent() {
        let entries = vec![
            entry("Firefox", None),
            entry("GoLand", None),
            entry("Steam", None),
        ];
        let mut counts = HashMap::new();
        counts.insert("Firefox".to_string(), 5);
        let history = LaunchHistory::from_counts(counts);
        let (recent, apps) = split_sections(entries, &history);
        assert_eq!(recent.iter().map(|e| &e.name).collect::<Vec<_>>(), vec!["Firefox"]);
        assert_eq!(
            apps.iter().map(|e| &e.name).collect::<Vec<_>>(),
            vec!["GoLand", "Steam"]
        );
    }

    #[test]
    fn split_sections_caps_recent_at_max() {
        let entries: Vec<DesktopEntry> = (0..(MAX_RECENT + 2))
            .map(|i| entry(&format!("App{i}"), None))
            .collect();
        let counts = entries
            .iter()
            .map(|e| (e.name.clone(), 1))
            .collect::<HashMap<_, _>>();
        let history = LaunchHistory::from_counts(counts);
        let (recent, apps) = split_sections(entries, &history);
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(apps.len(), 2);
    }

    #[test]
    fn split_sections_preserves_relative_order_within_each_group() {
        let mut counts = HashMap::new();
        counts.insert("A".to_string(), 1);
        counts.insert("C".to_string(), 3);
        let history = LaunchHistory::from_counts(counts);
        // load_sorted_entries would already have ordered these by count
        // desc before calling split_sections; split_sections itself just
        // partitions in whatever order it's handed, so feed it pre-sorted.
        let pre_sorted = vec![entry("C", None), entry("A", None), entry("B", None)];
        let (recent, apps) = split_sections(pre_sorted, &history);
        assert_eq!(recent.iter().map(|e| &e.name).collect::<Vec<_>>(), vec!["C", "A"]);
        assert_eq!(apps.iter().map(|e| &e.name).collect::<Vec<_>>(), vec!["B"]);
    }
}
