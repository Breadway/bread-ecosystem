//! Unix-user identities from a PolicyKit `BeginAuthentication` call.

/// A `unix-user` identity the agent can authenticate as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnixUser {
    pub uid: u32,
    pub name: String,
}

/// Look up `uid` in a passwd-file dump (`name:x:uid:...` lines).
pub fn name_for_uid(uid: u32, passwd: &str) -> Option<String> {
    for line in passwd.lines() {
        if line.starts_with('#') {
            continue;
        }
        let mut parts = line.split(':');
        let name = parts.next()?;
        let _pw = parts.next()?;
        let id = parts.next()?.parse::<u32>().ok()?;
        if id == uid && !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}

/// Resolve each uid to a [`UnixUser`], falling back to `uid N` when
/// `/etc/passwd` has no name.
pub fn users_from_uids(uids: &[u32], passwd: &str) -> Vec<UnixUser> {
    uids.iter()
        .copied()
        .map(|uid| UnixUser {
            uid,
            name: name_for_uid(uid, passwd).unwrap_or_else(|| format!("uid {uid}")),
        })
        .collect()
}

/// Prefer the process's own uid when it is in `users`, otherwise the first.
pub fn pick_user<'a>(users: &'a [UnixUser], current_uid: Option<u32>) -> Option<&'a UnixUser> {
    if let Some(uid) = current_uid {
        if let Some(user) = users.iter().find(|u| u.uid == uid) {
            return Some(user);
        }
    }
    users.first()
}

/// Real uid from a `/proc/self/status` dump (`Uid:\t<real> ...`).
pub fn uid_from_status(status: &str) -> Option<u32> {
    for line in status.lines() {
        let Some(rest) = line.strip_prefix("Uid:") else {
            continue;
        };
        return rest.split_whitespace().next()?.parse().ok();
    }
    None
}

/// Current real uid, or `None` if `/proc/self/status` is unreadable.
pub fn current_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    uid_from_status(&status)
}

/// Contents of `/etc/passwd`, or empty if unreadable.
pub fn read_passwd() -> String {
    std::fs::read_to_string("/etc/passwd").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "\
# comment
root:x:0:0:root:/root:/bin/sh
alice:x:1000:1000:Alice:/home/alice:/bin/zsh
bob:x:1001:1001:Bob:/home/bob:/bin/bash
";

    #[test]
    fn name_for_uid_reads_passwd_lines() {
        assert_eq!(name_for_uid(0, PASSWD).as_deref(), Some("root"));
        assert_eq!(name_for_uid(1000, PASSWD).as_deref(), Some("alice"));
        assert_eq!(name_for_uid(99, PASSWD), None);
    }

    #[test]
    fn users_from_uids_falls_back_to_uid_label() {
        let users = users_from_uids(&[1000, 42], PASSWD);
        assert_eq!(
            users,
            vec![
                UnixUser {
                    uid: 1000,
                    name: "alice".into()
                },
                UnixUser {
                    uid: 42,
                    name: "uid 42".into()
                },
            ]
        );
    }

    #[test]
    fn pick_user_prefers_current_uid() {
        let users = users_from_uids(&[0, 1000], PASSWD);
        let picked = pick_user(&users, Some(1000)).unwrap();
        assert_eq!(picked.name, "alice");
    }

    #[test]
    fn pick_user_falls_back_to_first() {
        let users = users_from_uids(&[0, 1000], PASSWD);
        let picked = pick_user(&users, Some(7)).unwrap();
        assert_eq!(picked.name, "root");
        assert!(pick_user(&[], Some(1000)).is_none());
    }

    #[test]
    fn uid_from_status_reads_real_uid() {
        let status = "Name:\tbread-polkit\nUid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(uid_from_status(status), Some(1000));
        assert_eq!(uid_from_status("Name:\tfoo\n"), None);
    }
}
