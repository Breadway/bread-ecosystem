use crate::track::Track;
use clap::builder::styling::{AnsiColor, Effects, Styles};
use std::io::{IsTerminal, Write};

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const CYAN: &str = "\x1b[36m";
pub const MAGENTA: &str = "\x1b[35m";
pub const BOLD_CYAN: &str = "\x1b[1;36m";

/// Clap help styling — same cyan headers / green literals / dim placeholders
/// as the rest of bakery, so `bakery --help` doesn't look like a different
/// program from `bakery list`.
pub const CLAP_STYLES: Styles = Styles::styled()
    .header(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::BrightBlack.on_default())
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD))
    .valid(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .invalid(AnsiColor::Yellow.on_default().effects(Effects::BOLD));

/// Colors are on only when stdout is a real terminal and `NO_COLOR` isn't
/// set — the ecosystem's existing CLI (breadcrumbs) hardcodes ANSI
/// unconditionally, which leaks escape codes into piped/logged output; this
/// is the hardening fix for that gap.
pub fn colors_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

pub fn colors_enabled_err() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal()
}

pub fn style(s: &str, code: &str) -> String {
    paint(s, code, colors_enabled())
}

fn style_err(s: &str, code: &str) -> String {
    paint(s, code, colors_enabled_err())
}

fn paint(s: &str, code: &str, on: bool) -> String {
    if on {
        format!("{code}{s}{RESET}")
    } else {
        s.to_string()
    }
}

pub fn bold(s: &str) -> String {
    style(s, BOLD)
}

pub fn dim(s: &str) -> String {
    style(s, DIM)
}

/// `" [beta]"` / `" [dev]"`, colored — empty string for `Stable` so the
/// common-case output is unchanged.
#[allow(dead_code)]
pub fn track_badge(track: Track) -> String {
    let tag = track_tag(track);
    if tag.is_empty() {
        tag
    } else {
        format!(" {tag}")
    }
}

/// `[beta]` / `[dev]` with no leading space; empty for `Stable`.
pub fn track_tag(track: Track) -> String {
    match track {
        Track::Stable => String::new(),
        Track::Beta => style("[beta]", YELLOW),
        Track::Dev => style("[dev]", MAGENTA),
    }
}

pub fn ok(s: &str) -> String {
    style(&format!("✓ {s}"), GREEN)
}

pub fn fail(s: &str) -> String {
    style(&format!("✗ {s}"), RED)
}

/// Neutral "nothing to do" glyph, dim rather than green — for steady-state
/// noise like "already at latest" in `bakery update --all`, where most
/// packages hit this every run. Reusing GREEN there drowns out the
/// packages that actually changed, and meaning shouldn't depend on color
/// alone (an unusual terminal palette can make BOLD/GREEN/DIM look similar),
/// so this also carries its own glyph the way `ok`/`fail` do.
pub fn unchanged(s: &str) -> String {
    style(&format!("· {s}"), DIM)
}

pub fn warn(s: &str) -> String {
    style(&format!("warning: {s}"), YELLOW)
}

pub fn note(s: &str) -> String {
    style(&format!("note: {s}"), DIM)
}

/// Cyan verb + bold name + dim version — the install/update/remove banner.
pub fn action(verb: &str, name: &str, version: Option<&str>) {
    let mut line = format!("{}  {}", style(verb, BOLD_CYAN), style(name, BOLD));
    if let Some(v) = version {
        line.push_str("  ");
        line.push_str(&style(v, DIM));
    }
    println!("{line}");
}

/// Section title plus dim meta (`Packages  16  ·  15 installed`).
pub fn heading(title: &str, parts: &[&str]) {
    let mut line = style(title, BOLD_CYAN);
    let visible: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
    for (i, part) in visible.iter().enumerate() {
        line.push_str("  ");
        if i > 0 {
            line.push_str(&style("·", DIM));
            line.push_str("  ");
        }
        line.push_str(part);
    }
    println!("{line}");
    println!();
}

pub fn summary(parts: &[&str]) {
    let visible: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
    if visible.is_empty() {
        return;
    }
    println!();
    println!("{}", style(&visible.join("  ·  "), BOLD));
}

/// Left-aligned verb column so install chatter (`downloading` / `placed` /
/// `unit`) lines up instead of drifting with the verb length.
pub fn step(verb: &str, detail: &str) {
    println!("  {:<12} {}", dim(verb), detail);
}

pub fn kv(key: &str, value: &str) {
    println!("  {:<12} {}", dim(key), value);
}

pub fn check_row(ok_flag: bool, name: &str, name_width: usize, message: &str) {
    let glyph = if ok_flag {
        style("✓", GREEN)
    } else {
        style("✗", RED)
    };
    println!("  {glyph}  {:<name_width$}  {message}", name);
}

pub fn unknown_row(name: &str, name_width: usize, message: &str) {
    println!(
        "  {}  {:<name_width$}  {}",
        style("?", DIM),
        name,
        dim(message)
    );
}

pub struct CatalogRow {
    pub name: String,
    pub version: String,
    pub installed: bool,
    /// Wrapped onto following lines (descriptions).
    pub detail: String,
    /// Same-line suffix after the version (short dates). Empty for catalog
    /// views that already use `detail`.
    pub aside: String,
}

/// Two-line catalog: status glyph + aligned name/version, then a hanging
/// description (or date) wrapped to the terminal width. Column widths are
/// computed from the row set so long `-dev.` versions no longer smash the
/// old `{: <10}` pad.
pub fn print_catalog(rows: &[CatalogRow]) {
    for line in format_catalog(rows, term_width()) {
        println!("{line}");
    }
}

pub fn format_catalog(rows: &[CatalogRow], width: usize) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let name_w = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let indent = 5; // "  ✓  " / "     "
    let detail_width = width.saturating_sub(indent).max(24);

    let mut lines = Vec::new();
    for row in rows {
        let glyph = if row.installed {
            style("✓", GREEN)
        } else {
            " ".to_string()
        };
        let name = style(&format!("{:<name_w$}", row.name), BOLD);
        let version = style(&row.version, DIM);
        let mut line = format!("  {glyph}  {name}  {version}");
        if !row.aside.is_empty() {
            line.push_str("  ");
            line.push_str(&dim(&row.aside));
        }
        lines.push(line);
        if !row.detail.is_empty() {
            for wrapped in wrap_words(&row.detail, detail_width) {
                lines.push(format!("     {}", dim(&wrapped)));
            }
        }
    }
    lines
}

pub fn wrap_words(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if cur.is_empty() {
            cur = word.to_string();
        } else if cur.len() + 1 + word.len() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

pub fn short_date(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| rfc3339.to_string())
}

pub fn name_width<S: AsRef<str>>(names: impl IntoIterator<Item = S>) -> usize {
    names
        .into_iter()
        .map(|s| s.as_ref().len())
        .max()
        .unwrap_or(0)
}

/// `\r`-overwritten download bar on stderr. Pads to a stable width so a
/// shorter later frame doesn't leave leftover characters from a longer one.
pub fn print_progress(downloaded: u64, total: u64) {
    let width = term_width().clamp(40, 72);
    let line = progress_line(downloaded, total, 20);
    let padded = fit_width(&line, width);
    eprint!("\r{padded}");
    let _ = std::io::stderr().flush();
}

pub fn finish_progress() {
    eprintln!();
}

pub fn progress_line(downloaded: u64, total: u64, bar_width: usize) -> String {
    let dl = downloaded as f64 / 1_048_576.0;
    let tot = total as f64 / 1_048_576.0;
    let frac = if total == 0 {
        0.0
    } else {
        (downloaded as f64 / total as f64).clamp(0.0, 1.0)
    };
    let filled = ((bar_width as f64) * frac).round() as usize;
    let filled = filled.min(bar_width);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(bar_width - filled));
    let pct = (frac * 100.0).round() as u32;
    format!(
        "  ⇣ {}  {:>3}%  {:.1}/{:.1} MB",
        style_err(&bar, CYAN),
        pct,
        dl,
        tot
    )
}

fn fit_width(s: &str, width: usize) -> String {
    let visible = visible_len(s);
    if visible >= width {
        return s.to_string();
    }
    format!("{s}{}", " ".repeat(width - visible))
}

fn visible_len(s: &str) -> usize {
    let mut n = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        n += 1;
    }
    n
}

pub fn term_width() -> usize {
    if let Ok(w) = std::env::var("COLUMNS") {
        if let Ok(n) = w.parse::<usize>() {
            if n >= 40 {
                return n;
            }
        }
    }
    ioctl_width().filter(|&n| n >= 40).unwrap_or(80)
}

#[cfg(unix)]
fn ioctl_width() -> Option<usize> {
    use std::os::fd::AsRawFd;

    #[repr(C)]
    struct WinSize {
        row: u16,
        col: u16,
        x: u16,
        y: u16,
    }

    unsafe extern "C" {
        fn ioctl(fd: i32, request: u64, argp: *mut WinSize) -> i32;
    }

    let mut ws = WinSize {
        row: 0,
        col: 0,
        x: 0,
        y: 0,
    };
    // TIOCGWINSZ on Linux.
    let fd = std::io::stdout().as_raw_fd();
    let ret = unsafe { ioctl(fd, 0x5413, &mut ws) };
    if ret == 0 && ws.col > 0 {
        Some(ws.col as usize)
    } else {
        None
    }
}

#[cfg(not(unix))]
fn ioctl_width() -> Option<usize> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_badge_is_empty() {
        assert_eq!(track_badge(Track::Stable), "");
    }

    #[test]
    fn dev_badge_is_nonempty() {
        assert!(!track_badge(Track::Dev).is_empty());
    }

    #[test]
    fn unchanged_carries_a_distinct_glyph_from_ok_and_fail() {
        // Meaning must survive even with colors stripped (NO_COLOR, or a
        // terminal palette that makes ANSI codes look alike) — so the glyph
        // itself has to differ, not just the color.
        assert!(unchanged("foo").contains('·'));
        assert!(!ok("foo").contains('·'));
        assert!(!fail("foo").contains('·'));
    }

    #[test]
    fn catalog_aligns_names_and_versions() {
        let lines = format_catalog(
            &[
                CatalogRow {
                    name: "bakery".into(),
                    version: "0.7.2-dev.20260815142350+30517f1".into(),
                    installed: true,
                    detail: "Package manager".into(),
                    aside: String::new(),
                },
                CatalogRow {
                    name: "breadarr".into(),
                    version: "0.1.2".into(),
                    installed: false,
                    detail: "Homelab arr stack".into(),
                    aside: String::new(),
                },
            ],
            80,
        );
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("bakery"));
        assert!(lines[0].contains("0.7.2-dev.20260815142350+30517f1"));
        assert!(lines[1].contains("Package manager"));
        // Shorter version is padded so the columns stay a block, not a
        // ragged list — the long bakery version used to overflow `{: <10}`.
        // Compare display columns, not byte offsets: the installed glyph
        // is a 3-byte checkmark sitting in a 1-column slot.
        let bakery_col = visible_len(&lines[0][..lines[0].find("0.7.2-dev").unwrap()]);
        let breadarr_col = visible_len(&lines[2][..lines[2].find("0.1.2").unwrap()]);
        assert_eq!(bakery_col, breadarr_col);
    }

    #[test]
    fn wrap_words_breaks_on_width() {
        let lines = wrap_words("one two three four", 9);
        assert_eq!(lines, vec!["one two", "three", "four"]);
    }

    #[test]
    fn progress_line_has_bar_and_percent() {
        let line = progress_line(1_048_576, 2_097_152, 10);
        assert!(line.contains('█'));
        assert!(line.contains('░'));
        assert!(line.contains("50%"));
        assert!(line.contains("1.0/2.0 MB"));
    }

    #[test]
    fn visible_len_ignores_ansi() {
        assert_eq!(visible_len("hello"), 5);
        assert_eq!(visible_len(&format!("{CYAN}hello{RESET}")), 5);
    }

    #[test]
    fn short_date_from_rfc3339() {
        assert_eq!(short_date("2026-08-15T14:23:50+00:00"), "2026-08-15");
    }
}
