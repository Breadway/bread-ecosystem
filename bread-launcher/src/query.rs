//! Query modes (`04-spotlight.html`'s `BOS.parseQuery`/`BOS.evalCalc`/
//! `BOS.COMMANDS`, THEME_SYSTEM_PLAN.md phase 6c): a leading `=`/`>`/`.`
//! switches the launcher from filtering apps to evaluating an arithmetic
//! expression, listing bread commands, or treating the rest of the query as
//! a URL to open. Pure/headless — no GTK here — so both breadbar's embedded
//! capsule and (per the module doc comment on `crate`) breadbox's own
//! overlay window can adopt the same parsing/eval/filter logic. A host is
//! expected to gate which prefixes it actually acts on against its theme's
//! `[launcher].modes` list — this module recognizes all four kinds
//! unconditionally and leaves that gating to the caller.

use crate::matching::fuzzy_matches;

/// Which of the four query modes a raw entry-text string names, per its
/// leading character (mirrors `BOS.parseQuery`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    /// No recognized prefix — the ordinary app-filter query.
    Apps,
    /// Leading `=` — the rest is an arithmetic expression for [`eval_calc`].
    Calc,
    /// Leading `>` — the rest filters [`builtin_commands`].
    Cmd,
    /// Leading `.` — the rest is a URL to open.
    Url,
}

/// A parsed query: which mode it names, and the text after the prefix
/// character (empty string for a bare `=`/`>`/`.` with nothing typed yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    pub kind: QueryKind,
    pub value: String,
}

/// Splits `raw` on its leading mode character, if any. Mirrors
/// `BOS.parseQuery` exactly: only the FIRST character is checked, and it is
/// stripped along with any immediately-following whitespace.
pub fn parse_query(raw: &str) -> ParsedQuery {
    let (kind, rest) = if let Some(rest) = raw.strip_prefix('=') {
        (QueryKind::Calc, rest)
    } else if let Some(rest) = raw.strip_prefix('>') {
        (QueryKind::Cmd, rest)
    } else if let Some(rest) = raw.strip_prefix('.') {
        (QueryKind::Url, rest)
    } else {
        (QueryKind::Apps, raw)
    };
    ParsedQuery {
        kind,
        value: rest.trim().to_string(),
    }
}

// ---- Calc ---------------------------------------------------------------

/// Evaluates `expr` as a small four-function arithmetic expression
/// (`+ - * / ( )`, decimal literals, unary minus) and formats the result
/// the same way `BOS.evalCalc` does: rounded to 8 decimal places, an
/// integer result prints with no trailing `.0`, a non-finite result prints
/// "∞", an expression containing anything outside `[0-9.\s+\-*/()]`
/// returns "bad expr", and anything else that fails to parse or evaluate
/// (unbalanced parens, division producing NaN, trailing garbage) returns
/// "err". `None` only for an empty/whitespace-only expression — the
/// caller's "nothing typed after `=` yet" case, which has no result to
/// show at all (the demo's own `if (!expr) return null;`).
pub fn eval_calc(expr: &str) -> Option<String> {
    let expr = expr.trim();
    if expr.is_empty() {
        return None;
    }
    if !expr.chars().all(|c| c.is_ascii_digit() || " .+-*/()".contains(c)) {
        return Some("bad expr".to_string());
    }
    let mut p = CalcParser {
        bytes: expr.as_bytes(),
        pos: 0,
    };
    let result = p.parse_expr().filter(|_| p.skip_ws() == p.bytes.len());
    Some(match result {
        Some(n) if !n.is_finite() => "∞".to_string(),
        Some(n) => format_calc_result(n),
        None => "err".to_string(),
    })
}

/// Rounds to 8 decimal places and formats without a trailing `.0` for whole
/// numbers — `String(Math.round(n * 1e8) / 1e8)` in JS, `{}` on `f64`
/// already behaves the same way in Rust (`format!("{}", 4.0_f64)` == "4").
fn format_calc_result(n: f64) -> String {
    let rounded = (n * 1e8).round() / 1e8;
    // Avoid printing "-0" for a result that rounds to negative zero.
    let rounded = if rounded == 0.0 { 0.0 } else { rounded };
    format!("{rounded}")
}

/// Minimal recursive-descent parser: `expr := term (('+'|'-') term)*`,
/// `term := factor (('*'|'/') factor)*`, `factor := '-' factor | number |
/// '(' expr ')'`. Byte-indexed since the character set is already
/// restricted to ASCII by [`eval_calc`]'s pre-check.
struct CalcParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> CalcParser<'a> {
    fn skip_ws(&mut self) -> usize {
        while self.pos < self.bytes.len() && self.bytes[self.pos] == b' ' {
            self.pos += 1;
        }
        self.pos
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    fn parse_expr(&mut self) -> Option<f64> {
        let mut val = self.parse_term()?;
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    val += self.parse_term()?;
                }
                Some(b'-') => {
                    self.pos += 1;
                    val -= self.parse_term()?;
                }
                _ => break,
            }
        }
        Some(val)
    }

    fn parse_term(&mut self) -> Option<f64> {
        let mut val = self.parse_factor()?;
        loop {
            match self.peek() {
                Some(b'*') => {
                    self.pos += 1;
                    val *= self.parse_factor()?;
                }
                Some(b'/') => {
                    self.pos += 1;
                    val /= self.parse_factor()?;
                }
                _ => break,
            }
        }
        Some(val)
    }

    fn parse_factor(&mut self) -> Option<f64> {
        match self.peek()? {
            b'-' => {
                self.pos += 1;
                Some(-self.parse_factor()?)
            }
            b'+' => {
                self.pos += 1;
                self.parse_factor()
            }
            b'(' => {
                self.pos += 1;
                let val = self.parse_expr()?;
                if self.peek() == Some(b')') {
                    self.pos += 1;
                    Some(val)
                } else {
                    None
                }
            }
            c if c.is_ascii_digit() || c == b'.' => self.parse_number(),
            _ => None,
        }
    }

    fn parse_number(&mut self) -> Option<f64> {
        self.skip_ws();
        let start = self.pos;
        let mut seen_dot = false;
        let mut seen_digit = false;
        while let Some(&c) = self.bytes.get(self.pos) {
            if c.is_ascii_digit() {
                seen_digit = true;
                self.pos += 1;
            } else if c == b'.' && !seen_dot {
                seen_dot = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        if !seen_digit {
            return None;
        }
        std::str::from_utf8(&self.bytes[start..self.pos])
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
    }
}

// ---- Commands -------------------------------------------------------------

/// One `>`-mode command palette entry: a display `name` and the shell
/// command it runs (via `bash -c`, same spawn convention [`crate::do_launch`]
/// already uses for a desktop entry's `Exec=` line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: &'static str,
    pub name: &'static str,
    pub exec: &'static str,
}

/// A small, real bread-ecosystem command palette — deliberately not the
/// demo's placeholder set (`Lock session`/`Open settings`/`Test
/// notification` map to nothing real in this codebase). `loginctl
/// lock-session` and `bread reload` are both already-documented, safe,
/// no-argument commands (see the bread CLI reference / systemd-logind).
pub fn builtin_commands() -> &'static [Command] {
    &[
        Command {
            id: "lock",
            name: "Lock session",
            exec: "loginctl lock-session",
        },
        Command {
            id: "reload-breadd",
            name: "Reload breadd",
            exec: "bread reload",
        },
    ]
}

/// Fuzzy-filters `commands` by `query` against each command's `name` (same
/// subsequence match [`crate::fuzzy_matches`] uses for app rows) — an empty
/// query matches everything, same as the app list's own empty-query case.
pub fn filter_commands(query: &str, commands: &[Command]) -> Vec<Command> {
    commands
        .iter()
        .filter(|c| fuzzy_matches(query, c.name))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_query -------------------------------------------------

    #[test]
    fn parse_query_bare_text_is_apps() {
        let p = parse_query("firefox");
        assert_eq!(p.kind, QueryKind::Apps);
        assert_eq!(p.value, "firefox");
    }

    #[test]
    fn parse_query_empty_is_apps() {
        let p = parse_query("");
        assert_eq!(p.kind, QueryKind::Apps);
        assert_eq!(p.value, "");
    }

    #[test]
    fn parse_query_equals_is_calc() {
        let p = parse_query("=2+2");
        assert_eq!(p.kind, QueryKind::Calc);
        assert_eq!(p.value, "2+2");
    }

    #[test]
    fn parse_query_gt_is_cmd() {
        let p = parse_query(">lock");
        assert_eq!(p.kind, QueryKind::Cmd);
        assert_eq!(p.value, "lock");
    }

    #[test]
    fn parse_query_dot_is_url() {
        let p = parse_query(".breadway.dev");
        assert_eq!(p.kind, QueryKind::Url);
        assert_eq!(p.value, "breadway.dev");
    }

    #[test]
    fn parse_query_strips_leading_whitespace_after_prefix() {
        let p = parse_query("=  2 + 2  ");
        assert_eq!(p.kind, QueryKind::Calc);
        assert_eq!(p.value, "2 + 2");
    }

    #[test]
    fn parse_query_bare_prefix_has_empty_value() {
        assert_eq!(parse_query("=").value, "");
        assert_eq!(parse_query(">").value, "");
        assert_eq!(parse_query(".").value, "");
    }

    // ---- eval_calc -----------------------------------------------------

    #[test]
    fn eval_calc_empty_expr_is_none() {
        assert_eq!(eval_calc(""), None);
        assert_eq!(eval_calc("   "), None);
    }

    #[test]
    fn eval_calc_simple_addition() {
        assert_eq!(eval_calc("2+2"), Some("4".to_string()));
    }

    #[test]
    fn eval_calc_precedence() {
        assert_eq!(eval_calc("2+3*4"), Some("14".to_string()));
    }

    #[test]
    fn eval_calc_parens() {
        assert_eq!(eval_calc("(2+3)*4"), Some("20".to_string()));
    }

    #[test]
    fn eval_calc_unary_minus() {
        assert_eq!(eval_calc("-5+2"), Some("-3".to_string()));
    }

    #[test]
    fn eval_calc_decimals_round_to_8_places() {
        assert_eq!(eval_calc("0.1+0.2"), Some("0.3".to_string()));
    }

    #[test]
    fn eval_calc_division_by_zero_is_infinity_symbol() {
        assert_eq!(eval_calc("1/0"), Some("∞".to_string()));
    }

    #[test]
    fn eval_calc_bad_chars_is_bad_expr() {
        assert_eq!(eval_calc("2+alert(1)"), Some("bad expr".to_string()));
        assert_eq!(eval_calc("rm -rf /"), Some("bad expr".to_string()));
    }

    #[test]
    fn eval_calc_unbalanced_parens_is_err() {
        assert_eq!(eval_calc("(2+3"), Some("err".to_string()));
    }

    #[test]
    fn eval_calc_trailing_garbage_is_err() {
        assert_eq!(eval_calc("2+3)"), Some("err".to_string()));
    }

    #[test]
    fn eval_calc_double_operator_is_err() {
        assert_eq!(eval_calc("2++"), Some("err".to_string()));
    }

    #[test]
    fn eval_calc_whitespace_is_tolerated() {
        assert_eq!(eval_calc(" 2  +  2 "), Some("4".to_string()));
    }

    // ---- commands --------------------------------------------------------

    #[test]
    fn builtin_commands_are_non_empty() {
        assert!(!builtin_commands().is_empty());
    }

    #[test]
    fn filter_commands_empty_query_matches_all() {
        let all = builtin_commands();
        assert_eq!(filter_commands("", all).len(), all.len());
    }

    #[test]
    fn filter_commands_filters_by_name_subsequence() {
        let matches = filter_commands("lock", builtin_commands());
        assert!(matches.iter().any(|c| c.id == "lock"));
        assert!(!matches.iter().any(|c| c.id == "reload-breadd"));
    }

    #[test]
    fn filter_commands_no_match_is_empty() {
        assert!(filter_commands("zzzznotacommand", builtin_commands()).is_empty());
    }
}
