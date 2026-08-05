use crate::track::Track;
use std::io::IsTerminal;

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const CYAN: &str = "\x1b[36m";
pub const MAGENTA: &str = "\x1b[35m";

/// Colors are on only when stdout is a real terminal and `NO_COLOR` isn't
/// set — the ecosystem's existing CLI (breadcrumbs) hardcodes ANSI
/// unconditionally, which leaks escape codes into piped/logged output; this
/// is the hardening fix for that gap.
pub fn colors_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

pub fn style(s: &str, code: &str) -> String {
    if colors_enabled() {
        format!("{code}{s}{RESET}")
    } else {
        s.to_string()
    }
}

/// `" [beta]"` / `" [dev]"`, colored — empty string for `Stable` so the
/// common-case output is unchanged.
pub fn track_badge(track: Track) -> String {
    match track {
        Track::Stable => String::new(),
        Track::Beta => format!(" {}", style("[beta]", YELLOW)),
        Track::Dev => format!(" {}", style("[dev]", MAGENTA)),
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
}
