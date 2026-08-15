//! App-id helpers shared by GTK tools and the singleton lock.
//!
//! The process / pid-file name (`breadbox`, `bread-polkit`) is also the
//! last segment of the GApplication id (`com.breadway.breadbox`). That is
//! *not* always the breadd command-bus id (`box`, `clip`) — see
//! [`crate::command_verb`] under feature `bread-client`.

use std::io;

use crate::singleton::{self, Acquire, Toggle};

/// Why [`parse_app_name`] / [`application_id`] rejected a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidAppId {
    /// The rejected input, owned so the error is `'static`.
    pub name: String,
    /// Short reason suitable for an `io::Error` / clap message.
    pub reason: &'static str,
}

impl std::fmt::Display for InvalidAppId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid app id '{}': {}", self.name, self.reason)
    }
}

impl std::error::Error for InvalidAppId {}

/// Accept a process / GTK application name (`breadbox`, `bread-polkit`).
///
/// Rules match a GApplication id *element*: non-empty, ASCII letter first,
/// then ASCII alphanumeric / `-` / `_`. Dots are rejected so the name can
/// sit in `com.breadway.<name>` without creating extra segments.
pub fn parse_app_name(name: &str) -> Result<&str, InvalidAppId> {
    if name.is_empty() {
        return Err(InvalidAppId {
            name: name.to_string(),
            reason: "must not be empty",
        });
    }
    let mut chars = name.chars();
    let first = chars.next().expect("non-empty");
    if !first.is_ascii_alphabetic() {
        return Err(InvalidAppId {
            name: name.to_string(),
            reason: "must start with an ASCII letter",
        });
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(InvalidAppId {
            name: name.to_string(),
            reason: "only ASCII letters, digits, '-' and '_' are allowed",
        });
    }
    Ok(name)
}

/// Reverse-DNS GApplication id: `com.breadway.<name>`.
pub fn application_id(app_name: &str) -> Result<String, InvalidAppId> {
    let name = parse_app_name(app_name)?;
    Ok(format!("com.breadway.{name}"))
}

/// [`singleton::try_acquire`] after [`parse_app_name`].
///
/// Invalid names become [`io::ErrorKind::InvalidInput`] and never touch
/// the pid file.
pub fn try_acquire(app_name: &str) -> io::Result<Acquire> {
    let name =
        parse_app_name(app_name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    singleton::try_acquire(name)
}

/// [`singleton::toggle_or_kill`] after [`parse_app_name`].
pub fn toggle_or_kill(app_name: &str) -> io::Result<Toggle> {
    let name =
        parse_app_name(app_name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    singleton::toggle_or_kill(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_app_name_accepts_existing_tool_names() {
        for name in ["breadbox", "breadclip", "bread-polkit", "breadcast"] {
            assert_eq!(parse_app_name(name), Ok(name));
        }
    }

    #[test]
    fn parse_app_name_rejects_empty_dot_and_leading_digit() {
        assert!(parse_app_name("").is_err());
        assert!(parse_app_name("bread.box").is_err());
        assert!(parse_app_name("1box").is_err());
        assert!(parse_app_name("-box").is_err());
        assert!(parse_app_name("bread box").is_err());
    }

    #[test]
    fn application_id_uses_com_breadway_prefix() {
        assert_eq!(application_id("breadbox").unwrap(), "com.breadway.breadbox");
        assert_eq!(
            application_id("bread-polkit").unwrap(),
            "com.breadway.bread-polkit"
        );
    }

    #[test]
    fn application_id_rejects_invalid_name() {
        assert!(application_id("").is_err());
        assert!(application_id("bread.box").is_err());
    }

    #[test]
    fn try_acquire_rejects_invalid_name_before_lock() {
        match try_acquire("") {
            Err(err) => assert_eq!(err.kind(), io::ErrorKind::InvalidInput),
            Ok(_) => panic!("empty name must not acquire a lock"),
        }
        match try_acquire("bread.box") {
            Err(err) => assert_eq!(err.kind(), io::ErrorKind::InvalidInput),
            Ok(_) => panic!("dotted name must not acquire a lock"),
        }
    }

    #[test]
    fn try_acquire_accepts_valid_name() {
        let name = format!("bread-app-id-test-{}", std::process::id());
        match try_acquire(&name).unwrap() {
            Acquire::Acquired(_guard) => {}
            Acquire::HeldByOther(_) => panic!("expected first acquire to succeed"),
        }
    }

    #[test]
    fn toggle_or_kill_starts_when_nothing_else_is_running() {
        let name = format!("bread-app-toggle-test-{}", std::process::id());
        match toggle_or_kill(&name).unwrap() {
            Toggle::Started(_guard) => {}
            Toggle::KilledExisting => panic!("expected to start as the first instance"),
        }
    }
}
