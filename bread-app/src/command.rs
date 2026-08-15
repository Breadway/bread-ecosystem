//! Command-bus helpers for `bread.command.<app>.**`.
//!
//! The `command_id` here is the breadd sibling-app id (`clip`, `box`,
//! `shot`) — often shorter than the GTK / singleton name (`breadclip`).

use crate::id::{parse_app_name, InvalidAppId};
use bread_utils::bread_client::{BreadClient, BreadEvent, Subscription};

/// Same charset as [`parse_app_name`]: a single command-bus segment.
pub fn parse_command_id(command_id: &str) -> Result<&str, InvalidAppId> {
    parse_app_name(command_id)
}

/// Subscribe glob: `bread.command.<app>.**`.
pub fn command_pattern(command_id: &str) -> Result<String, InvalidAppId> {
    let id = parse_command_id(command_id)?;
    Ok(format!("bread.command.{id}.**"))
}

/// The verb segment of `bread.command.<app>.<verb>` (and extra trailing
/// segments, if any). `None` when the event is not addressed to
/// `command_id` or the verb is missing.
///
/// Extra dotted remainder (`bread.command.clip.stack.clear`) yields the
/// first remaining segment (`stack`) — a verb is one segment, matching
/// [`BreadClient::command`].
pub fn command_verb<'a>(event: &'a str, command_id: &str) -> Option<&'a str> {
    if command_id.is_empty() {
        return None;
    }
    let prefix = format!("bread.command.{command_id}.");
    let rest = event.strip_prefix(&prefix)?;
    let verb = rest.split('.').next()?;
    if verb.is_empty() {
        None
    } else {
        Some(verb)
    }
}

/// Subscribe to `bread.command.<command_id>.**` and invoke `on_verb` with
/// the parsed verb plus the raw event.
///
/// Fail-silent: constructing the client and holding the subscription never
/// requires breadd to be running. Drop the returned [`Subscription`] (or
/// call [`Subscription::stop`]) to end the loop.
pub fn listen_commands<F>(command_id: &str, on_verb: F) -> Result<Subscription, InvalidAppId>
where
    F: Fn(&str, BreadEvent) + Send + 'static,
{
    let id = parse_command_id(command_id)?.to_string();
    let client = BreadClient::connect(id.clone());
    let pattern = format!("bread.command.{id}.**");
    Ok(client.subscribe(pattern, move |event| {
        let Some(verb) = command_verb(&event.event, &id).map(str::to_owned) else {
            return;
        };
        on_verb(&verb, event);
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_pattern_uses_double_star() {
        assert_eq!(command_pattern("clip").unwrap(), "bread.command.clip.**");
        assert_eq!(command_pattern("shot").unwrap(), "bread.command.shot.**");
    }

    #[test]
    fn command_pattern_rejects_invalid_id() {
        assert!(command_pattern("").is_err());
        assert!(command_pattern("clip.clear").is_err());
    }

    #[test]
    fn command_verb_strips_app_prefix() {
        assert_eq!(
            command_verb("bread.command.clip.clear", "clip"),
            Some("clear")
        );
        assert_eq!(
            command_verb("bread.command.shot.region", "shot"),
            Some("region")
        );
        assert_eq!(
            command_verb("bread.command.shot.annotate", "shot"),
            Some("annotate")
        );
    }

    #[test]
    fn command_verb_takes_first_segment_only() {
        assert_eq!(
            command_verb("bread.command.clip.stack.clear", "clip"),
            Some("stack")
        );
    }

    #[test]
    fn command_verb_rejects_other_apps_and_missing_verb() {
        assert_eq!(command_verb("bread.command.clip.clear", "shot"), None);
        assert_eq!(command_verb("bread.command.clip", "clip"), None);
        assert_eq!(command_verb("bread.command.clip.", "clip"), None);
        assert_eq!(command_verb("bread.clip.copied", "clip"), None);
        assert_eq!(command_verb("bread.command.clip.clear", ""), None);
    }

    #[test]
    fn listen_commands_rejects_invalid_id() {
        assert!(listen_commands("", |_, _| {}).is_err());
    }

    #[test]
    fn listen_commands_stop_joins_without_a_daemon() {
        let sub = listen_commands("clip", |_, _| {}).unwrap();
        sub.stop();
    }
}
