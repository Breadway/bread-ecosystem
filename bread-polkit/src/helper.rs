//! `polkit-agent-helper-1` transport and PAM line parser.
//!
//! Arch polkit 127+ talks over `/run/polkit/agent-helper.socket`. Older
//! builds still spawn the setuid helper at
//! `/usr/lib/polkit-1/polkit-agent-helper-1`. Prefer the socket when it
//! exists.

use std::path::{Path, PathBuf};

/// How this agent will talk to polkit's helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport {
    /// systemd socket-activated helper (polkit 127+).
    Socket(PathBuf),
    /// Legacy setuid helper binary.
    Exec(PathBuf),
}

const SOCKET_CANDIDATES: &[&str] = &["/run/polkit/agent-helper.socket"];
const HELPER_CANDIDATES: &[&str] = &[
    "/usr/lib/polkit-1/polkit-agent-helper-1",
    "/usr/libexec/polkit-1/polkit-agent-helper-1",
];

/// One stdout line from the helper after the cookie handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperLine {
    PromptEchoOff(String),
    PromptEchoOn(String),
    ErrorMsg(String),
    TextInfo(String),
    Success,
    Failure,
    Other(String),
}

/// Pick a live transport: `BREAD_POLKIT_SOCKET` / `BREAD_POLKIT_HELPER`
/// if set and present, otherwise the first existing well-known path.
pub fn discover_transport() -> Option<Transport> {
    discover_transport_from(
        std::env::var_os("BREAD_POLKIT_SOCKET")
            .map(PathBuf::from)
            .as_deref(),
        std::env::var_os("BREAD_POLKIT_HELPER")
            .map(PathBuf::from)
            .as_deref(),
        SOCKET_CANDIDATES,
        HELPER_CANDIDATES,
        |p| p.exists(),
    )
}

/// Testable discovery: `exists` is injected so unit tests do not need a
/// real `/run/polkit` socket.
pub fn discover_transport_from(
    socket_override: Option<&Path>,
    helper_override: Option<&Path>,
    sockets: &[&str],
    helpers: &[&str],
    exists: impl Fn(&Path) -> bool,
) -> Option<Transport> {
    if let Some(path) = socket_override {
        if exists(path) {
            return Some(Transport::Socket(path.to_path_buf()));
        }
    }
    for candidate in sockets {
        let path = Path::new(candidate);
        if exists(path) {
            return Some(Transport::Socket(path.to_path_buf()));
        }
    }
    if let Some(path) = helper_override {
        if exists(path) {
            return Some(Transport::Exec(path.to_path_buf()));
        }
    }
    for candidate in helpers {
        let path = Path::new(candidate);
        if exists(path) {
            return Some(Transport::Exec(path.to_path_buf()));
        }
    }
    None
}

/// Parse one helper protocol line. Prefix match is case-sensitive and
/// matches polkit's own `PAM_*` / `SUCCESS` / `FAILURE` tokens.
pub fn parse_helper_line(line: &str) -> HelperLine {
    let line = line.trim_end_matches(['\r', '\n']);
    if line == "SUCCESS" || line.starts_with("SUCCESS") {
        return HelperLine::Success;
    }
    if line == "FAILURE" || line.starts_with("FAILURE") {
        return HelperLine::Failure;
    }
    if let Some(rest) = line.strip_prefix("PAM_PROMPT_ECHO_OFF") {
        return HelperLine::PromptEchoOff(rest.trim().to_string());
    }
    if let Some(rest) = line.strip_prefix("PAM_PROMPT_ECHO_ON") {
        return HelperLine::PromptEchoOn(rest.trim().to_string());
    }
    if let Some(rest) = line.strip_prefix("PAM_ERROR_MSG") {
        return HelperLine::ErrorMsg(rest.trim().to_string());
    }
    if let Some(rest) = line.strip_prefix("PAM_TEXT_INFO") {
        return HelperLine::TextInfo(rest.trim().to_string());
    }
    HelperLine::Other(line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn parse_helper_line_known_tokens() {
        assert_eq!(parse_helper_line("SUCCESS"), HelperLine::Success);
        assert_eq!(parse_helper_line("SUCCESS\n"), HelperLine::Success);
        assert_eq!(parse_helper_line("FAILURE"), HelperLine::Failure);
        assert_eq!(
            parse_helper_line("PAM_PROMPT_ECHO_OFF Password:"),
            HelperLine::PromptEchoOff("Password:".into())
        );
        assert_eq!(
            parse_helper_line("PAM_PROMPT_ECHO_OFF"),
            HelperLine::PromptEchoOff(String::new())
        );
        assert_eq!(
            parse_helper_line("PAM_PROMPT_ECHO_ON login:"),
            HelperLine::PromptEchoOn("login:".into())
        );
        assert_eq!(
            parse_helper_line("PAM_ERROR_MSG Authentication failure"),
            HelperLine::ErrorMsg("Authentication failure".into())
        );
        assert_eq!(
            parse_helper_line("PAM_TEXT_INFO Account locked"),
            HelperLine::TextInfo("Account locked".into())
        );
        assert_eq!(
            parse_helper_line("garbage"),
            HelperLine::Other("garbage".into())
        );
    }

    #[test]
    fn discover_prefers_socket_over_exec() {
        let present: HashSet<PathBuf> = [
            "/run/polkit/agent-helper.socket",
            "/usr/lib/polkit-1/polkit-agent-helper-1",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        let got = discover_transport_from(None, None, SOCKET_CANDIDATES, HELPER_CANDIDATES, |p| {
            present.contains(p)
        });
        assert_eq!(
            got,
            Some(Transport::Socket(PathBuf::from(
                "/run/polkit/agent-helper.socket"
            )))
        );
    }

    #[test]
    fn discover_falls_back_to_helper_binary() {
        let present: HashSet<PathBuf> = ["/usr/lib/polkit-1/polkit-agent-helper-1"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let got = discover_transport_from(None, None, SOCKET_CANDIDATES, HELPER_CANDIDATES, |p| {
            present.contains(p)
        });
        assert_eq!(
            got,
            Some(Transport::Exec(PathBuf::from(
                "/usr/lib/polkit-1/polkit-agent-helper-1"
            )))
        );
    }

    #[test]
    fn discover_override_socket_wins_when_present() {
        let override_path = Path::new("/tmp/bread-polkit-test.sock");
        let got = discover_transport_from(
            Some(override_path),
            None,
            SOCKET_CANDIDATES,
            HELPER_CANDIDATES,
            |p| p == override_path,
        );
        assert_eq!(got, Some(Transport::Socket(override_path.to_path_buf())));
    }

    #[test]
    fn discover_none_when_nothing_exists() {
        let got =
            discover_transport_from(None, None, SOCKET_CANDIDATES, HELPER_CANDIDATES, |_| false);
        assert_eq!(got, None);
    }
}
