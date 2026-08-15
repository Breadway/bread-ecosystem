//! Session subject for `RegisterAuthenticationAgent`.

/// Logind session id from `XDG_SESSION_ID`, falling back to
/// `/proc/self/sessionid` when the kernel has one.
pub fn session_id() -> Option<String> {
    let xdg = std::env::var("XDG_SESSION_ID").ok();
    let proc = std::fs::read_to_string("/proc/self/sessionid").ok();
    session_id_from(xdg.as_deref(), proc.as_deref())
}

/// `None` when both sources are empty or the kernel reports the
/// unsigned `-1` sentinel (`4294967295`) meaning "no session".
pub fn session_id_from(xdg: Option<&str>, proc_sessionid: Option<&str>) -> Option<String> {
    if let Some(id) = xdg.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(id.to_string());
    }
    let raw = proc_sessionid?.trim();
    if raw.is_empty() || raw == "4294967295" {
        return None;
    }
    Some(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_xdg_session_id() {
        assert_eq!(session_id_from(Some("3"), Some("7")).as_deref(), Some("3"));
        assert_eq!(
            session_id_from(Some(" 3 "), Some("7")).as_deref(),
            Some("3")
        );
    }

    #[test]
    fn falls_back_to_proc_sessionid() {
        assert_eq!(session_id_from(Some(""), Some("7")).as_deref(), Some("7"));
        assert_eq!(session_id_from(None, Some("7\n")).as_deref(), Some("7"));
    }

    #[test]
    fn rejects_unset_kernel_session() {
        assert_eq!(session_id_from(None, Some("4294967295")), None);
        assert_eq!(session_id_from(Some(""), Some("")), None);
        assert_eq!(session_id_from(None, None), None);
    }
}
