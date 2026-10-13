//! Client-side redaction for everything breadbug sends: panic messages,
//! locations, module paths and log lines.
//!
//! It hides the home directory, the user name, `/home/<name>`-style roots,
//! e-mail addresses, IPv4 addresses, well-known token prefixes, `key=value`
//! pairs whose key looks secret, and long hex identifiers. Redaction is a
//! denylist by nature, so this is the first of three layers, not the last:
//! breadd scrubs with an allowlist on ingest, and the report is scrubbed again
//! at send time. The client never forwards the environment: `HOME` and the
//! user name are read only to know what to hide.

use std::sync::OnceLock;

const ROOTS: [(&str, &str); 3] = [
    ("/home/", "/home/<user>"),
    ("/Users/", "/Users/<user>"),
    ("/run/user/", "/run/user/<uid>"),
];
const SECRET_PREFIXES: [&str; 8] = ["sk-", "sk_", "ghp_", "gho_", "ghs_", "glpat-", "xox", "AKIA"];
const SENSITIVE_KEYS: [&str; 6] = ["pass", "secret", "token", "key", "auth", "cookie"];

/// Redacts text using a known home directory and user name.
#[derive(Debug, Clone, Default)]
pub struct Scrubber {
    home: Option<String>,
    user: Option<String>,
}

impl Scrubber {
    /// A scrubber for an explicit home directory and user name.
    pub fn new(home: Option<&str>, user: Option<&str>) -> Self {
        Self {
            home: home.map(str::to_owned),
            user: user.map(str::to_owned),
        }
    }

    /// A scrubber for the current `HOME` and user name.
    pub fn from_env() -> Self {
        let home = std::env::var("HOME").ok();
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .ok();
        Self::new(home.as_deref(), user.as_deref())
    }

    /// `text` with home directories, user names, e-mail addresses, IPv4
    /// addresses, token-looking strings and long hex identifiers replaced.
    pub fn redact(&self, text: &str) -> String {
        redact_with(text, self.home.as_deref(), self.user.as_deref())
    }
}

/// Redact `text` with an explicit home directory and user name.
pub fn redact_with(text: &str, home: Option<&str>, user: Option<&str>) -> String {
    let mut s = text.to_string();
    if let Some(h) = home
        .map(|h| h.trim_end_matches('/'))
        .filter(|h| h.len() > 1 && h.starts_with('/'))
    {
        s = s.replace(h, "~");
    }
    s = redact_roots(&s);
    // Whole-token rules first, so an e-mail or token that contains the user
    // name is classed as a unit, not split around the name.
    s = redact_words(&s);
    if let Some(u) = user.filter(|u| u.len() >= 3) {
        s = replace_word(&s, u, "<user>");
    }
    s
}

fn redact_roots(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some((at, prefix, label)) = ROOTS
        .iter()
        .filter_map(|(p, l)| rest.find(p).map(|i| (i, *p, *l)))
        .min_by_key(|t| t.0)
    {
        out.push_str(&rest[..at]);
        let after = &rest[at + prefix.len()..];
        let end = after
            .find(|c: char| c == '/' || c.is_whitespace() || "\"':),;".contains(c))
            .unwrap_or(after.len());
        out.push_str(label);
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn replace_word(s: &str, needle: &str, with: &str) -> String {
    let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for (at, _) in s.match_indices(needle) {
        let before = s[..at].chars().next_back();
        let after = s[at + needle.len()..].chars().next();
        if !is_word(before) && !is_word(after) {
            out.push_str(&s[last..at]);
            out.push_str(with);
            last = at + needle.len();
        }
    }
    out.push_str(&s[last..]);
    out
}

fn is_separator(c: char) -> bool {
    c.is_whitespace() || "\"'`()[]{}<>,;".contains(c)
}

fn redact_words(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut word = String::new();
    for c in s.chars() {
        if is_separator(c) {
            out.push_str(&redact_word(&word));
            word.clear();
            out.push(c);
        } else {
            word.push(c);
        }
    }
    out.push_str(&redact_word(&word));
    out
}

fn redact_word(w: &str) -> String {
    if w.is_empty() {
        return String::new();
    }
    if is_email(w) {
        return "<email>".into();
    }
    if w.len() >= 12 && SECRET_PREFIXES.iter().any(|p| w.starts_with(p)) {
        return "<secret>".into();
    }
    if let Some((key, _)) = w.split_once('=') {
        let lower = key.to_ascii_lowercase();
        if SENSITIVE_KEYS.iter().any(|s| lower.contains(s)) {
            return format!("{key}=<redacted>");
        }
    }
    if is_ipv4(w) {
        return "<ip>".into();
    }
    if w.len() >= 32 && w.bytes().all(|b| b.is_ascii_hexdigit()) {
        return "<id>".into();
    }
    w.to_string()
}

fn is_email(w: &str) -> bool {
    let Some((local, domain)) = w.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
}

fn is_ipv4(w: &str) -> bool {
    let parts: Vec<&str> = w.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u16>().is_ok_and(|v| v <= 255)
        })
}

/// The scrubber for the running process, read once and then shared.
pub fn process_default() -> &'static Scrubber {
    static DEFAULT: OnceLock<Scrubber> = OnceLock::new();
    DEFAULT.get_or_init(Scrubber::from_env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_user_and_home_roots_are_hidden() {
        assert_eq!(
            redact_with(
                "panic at /home/alice/proj/src/main.rs:3:4 and /home/alice/.cache",
                Some("/home/alice"),
                Some("alice"),
            ),
            "panic at ~/proj/src/main.rs:3:4 and ~/.cache"
        );
        assert_eq!(
            redact_with("/home/bob/x and /Users/carol/y", None, None),
            "/home/<user>/x and /Users/<user>/y"
        );
        assert_eq!(
            redact_with("hello alice, alicia", None, Some("alice")),
            "hello <user>, alicia"
        );
    }

    #[test]
    fn emails_tokens_addresses_and_secret_pairs_are_hidden() {
        assert_eq!(
            redact_with(
                "mail alice@example.com token sk-live-ABCDEF1234567890 ip 192.168.1.10 password=hunter2 ok",
                None,
                None,
            ),
            "mail <email> token <secret> ip <ip> password=<redacted> ok"
        );
    }

    #[test]
    fn long_hex_identifiers_are_hidden_but_short_ones_are_not() {
        assert_eq!(
            redact_with("serial 0123456789abcdef0123456789abcdef build cafe", None, None),
            "serial <id> build cafe"
        );
    }

    #[test]
    fn plain_text_and_locations_pass_through() {
        assert_eq!(
            redact_with("index out of bounds: len 3", None, None),
            "index out of bounds: len 3"
        );
        assert_eq!(
            redact_with("src/app.rs:10:5", Some("/home/alice"), Some("alice")),
            "src/app.rs:10:5"
        );
    }

    #[test]
    fn scrubber_applies_its_own_home_and_user() {
        let s = Scrubber::new(Some("/home/zed-tester"), Some("zed-tester"));
        assert_eq!(
            s.redact("/home/zed-tester/diary zed-tester@example.com"),
            "~/diary <email>"
        );
    }
}
