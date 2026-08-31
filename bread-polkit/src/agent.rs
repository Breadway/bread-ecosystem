//! Session-bus registration and the PolicyKit1 AuthenticationAgent.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use gtk4::glib;
use gtk4::prelude::*;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
use zbus::zvariant::{OwnedValue, Type, Value};
use zbus::{connection, interface, proxy, DBusError};

use bread_polkit::helper::{discover_transport, Transport};
use bread_polkit::identity::{current_uid, pick_user, read_passwd, users_from_uids, UnixUser};
use bread_polkit::session::session_id;

use crate::auth::{self, Outcome};
use crate::ui::{self, Prompt};

pub const OBJECT_PATH: &str = "/com/breadway/PolicyKit1/AuthenticationAgent";

/// Reply from the GTK prompt.
#[derive(Debug)]
pub enum UserAction {
    Submit { username: String, password: String },
    Cancel,
}

#[derive(Debug, DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Failed(String),
    Cancelled(String),
}

#[derive(Debug, Deserialize, Serialize, Type)]
struct Identity {
    kind: String,
    details: HashMap<String, OwnedValue>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Type)]
struct Subject {
    kind: String,
    details: HashMap<String, OwnedValue>,
}

#[proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority"
)]
trait Authority {
    fn register_authentication_agent(
        &self,
        subject: &Subject,
        locale: &str,
        object_path: &str,
    ) -> zbus::Result<()>;

    fn unregister_authentication_agent(
        &self,
        subject: &Subject,
        object_path: &str,
    ) -> zbus::Result<()>;
}

struct Agent {
    transport: Transport,
    pending: Arc<Mutex<Option<mpsc::Sender<UserAction>>>>,
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    async fn begin_authentication(
        &mut self,
        action_id: String,
        message: String,
        _icon_name: String,
        _details: HashMap<String, String>,
        cookie: String,
        identities: Vec<Identity>,
    ) -> Result<(), AgentError> {
        tracing::info!(%action_id, %cookie, "BeginAuthentication");

        let users = unix_users(&identities);
        let username = pick_user(&users, current_uid())
            .map(|u| u.name.clone())
            .ok_or_else(|| AgentError::Failed("no unix-user identity".into()))?;
        let allowed_users: Vec<String> = users.iter().map(|u| u.name.clone()).collect();

        let (tx, mut rx) = mpsc::channel(4);
        *self.pending.lock().await = Some(tx.clone());

        let prompt = Prompt {
            cookie: cookie.clone(),
            message: message.clone(),
            action_id: action_id.clone(),
            username: username.clone(),
            reply: tx,
        };
        invoke_ui(move || {
            if let Some(app) = running_app() {
                ui::show_prompt(&app, prompt);
            }
        });

        let result = self.drive_prompt(&cookie, &username, &allowed_users, &mut rx).await;

        *self.pending.lock().await = None;
        let cookie_close = cookie.clone();
        invoke_ui(move || ui::close_prompt(&cookie_close));
        result
    }

    async fn cancel_authentication(&self, cookie: String) {
        tracing::info!(%cookie, "CancelAuthentication");
        if let Some(tx) = self.pending.lock().await.as_ref() {
            let _ = tx.try_send(UserAction::Cancel);
        }
        invoke_ui(move || ui::close_prompt(&cookie));
    }
}

impl Agent {
    async fn drive_prompt(
        &self,
        cookie: &str,
        default_user: &str,
        allowed_users: &[String],
        rx: &mut mpsc::Receiver<UserAction>,
    ) -> Result<(), AgentError> {
        loop {
            match rx.recv().await {
                None => {
                    return Err(AgentError::Cancelled("authentication prompt closed".into()));
                }
                Some(UserAction::Cancel) => {
                    return Err(AgentError::Cancelled("user cancelled".into()));
                }
                Some(UserAction::Submit { username, password }) => {
                    // The username field is user-editable; only accept it when
                    // it's one of the identities polkit offered (empty falls
                    // back to the prefilled user). Anything else is rejected
                    // and the prompt re-shown rather than starting a PAM
                    // conversation for an account the request never offered.
                    let Some(user) = resolve_user(default_user, allowed_users, &username) else {
                        let cookie = cookie.to_string();
                        invoke_ui(move || ui::show_retry(&cookie, INVALID_USER_MESSAGE));
                        continue;
                    };
                    match auth::authenticate(&self.transport, &user, cookie, &password).await {
                        Ok(Outcome::Success) => return Ok(()),
                        Ok(Outcome::Failure { message }) => {
                            let text = message
                                .unwrap_or_else(|| auth::default_failure_message().to_string());
                            let cookie = cookie.to_string();
                            invoke_ui(move || ui::show_retry(&cookie, &text));
                        }
                        Err(e) => {
                            tracing::warn!("helper: {e:#}");
                            let text = e.to_string();
                            let cookie = cookie.to_string();
                            invoke_ui(move || ui::show_retry(&cookie, &text));
                        }
                    }
                }
            }
        }
    }
}

const INVALID_USER_MESSAGE: &str =
    "That user is not one of the identities this request offered — use the prefilled user.";

/// Choose the username to authenticate as from the prompt input.
///
/// Empty input falls back to `default`. Non-empty input must be one of
/// `allowed` — the `unix-user` identities polkit actually offered in
/// `BeginAuthentication` — otherwise it returns `None`. Without this, a
/// request scoped to specific accounts (say, `root` only) could have its
/// editable username field redirected to an arbitrary local user, kicking
/// off a PAM conversation for an account the request never offered.
/// (`auth::authenticate`'s PAM result still has to clear polkit's own
/// authorization, but this closes the obvious foot-gun at the agent — the
/// one place the identity list is actually known.)
fn resolve_user(default: &str, allowed: &[String], input: &str) -> Option<String> {
    if input.is_empty() {
        return Some(default.to_string());
    }
    if allowed.iter().any(|u| u.as_str() == input) {
        return Some(input.to_string());
    }
    None
}

fn unix_users(identities: &[Identity]) -> Vec<UnixUser> {
    let mut uids = Vec::new();
    for identity in identities {
        if identity.kind != "unix-user" {
            continue;
        }
        if let Some(uid) = uid_from_details(&identity.details) {
            uids.push(uid);
        }
    }
    users_from_uids(&uids, &read_passwd())
}

fn uid_from_details(details: &HashMap<String, OwnedValue>) -> Option<u32> {
    let value = details.get("uid")?;
    u32::try_from(value).ok().or_else(|| {
        i32::try_from(value)
            .ok()
            .and_then(|n| u32::try_from(n).ok())
    })
}

fn running_app() -> Option<gtk4::Application> {
    gtk4::gio::Application::default().and_then(|app| app.downcast::<gtk4::Application>().ok())
}

/// GTK thread-default context, captured in [`spawn`] so the dbus thread
/// can `invoke` onto the UI thread instead of its own empty context.
static GTK_CTX: OnceLock<glib::MainContext> = OnceLock::new();

fn invoke_ui(f: impl FnOnce() + Send + 'static) {
    let ctx = GTK_CTX
        .get()
        .cloned()
        .unwrap_or_else(glib::MainContext::default);
    ctx.invoke(f);
}

fn unix_session_subject(id: &str) -> Result<Subject> {
    let value = Value::from(id.to_string());
    let owned = OwnedValue::try_from(value).context("session-id variant")?;
    let mut details = HashMap::new();
    details.insert("session-id".into(), owned);
    Ok(Subject {
        kind: "unix-session".into(),
        details,
    })
}

/// Spawn the system-bus agent on a background thread. Returns once the
/// thread has been started; registration errors quit the GTK app.
///
/// Must be called from the GTK thread so the main context we capture is
/// the one driving the password prompt.
pub fn spawn() -> Result<()> {
    let _ = GTK_CTX.set(glib::MainContext::default());
    std::thread::Builder::new()
        .name("bread-polkit-dbus".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    invoke_ui(move || {
                        eprintln!("bread-polkit: tokio runtime failed: {e}");
                        if let Some(app) = running_app() {
                            app.quit();
                        }
                    });
                    return;
                }
            };
            rt.block_on(async move {
                if let Err(e) = run().await {
                    eprintln!("bread-polkit: {e:#}");
                    invoke_ui(|| {
                        if let Some(app) = running_app() {
                            app.quit();
                        }
                    });
                }
            });
        })
        .context("spawn dbus thread")?;
    Ok(())
}

async fn run() -> Result<()> {
    let transport = discover_transport().context(
        "no polkit helper: expected /run/polkit/agent-helper.socket \
         or /usr/lib/polkit-1/polkit-agent-helper-1",
    )?;
    tracing::info!(?transport, "using polkit helper");

    let session = session_id().context(
        "no session id (XDG_SESSION_ID / /proc/self/sessionid); \
         cannot register a session authentication agent",
    )?;
    let subject = unix_session_subject(&session)?;
    let locale = std::env::var("LANG").unwrap_or_else(|_| "C".into());

    let agent = Agent {
        transport,
        pending: Arc::new(Mutex::new(None)),
    };

    let connection = connection::Builder::system()?
        .serve_at(OBJECT_PATH, agent)?
        .build()
        .await
        .context("system bus")?;

    let authority = AuthorityProxy::new(&connection)
        .await
        .context("PolicyKit1 authority proxy")?;
    authority
        .register_authentication_agent(&subject, &locale, OBJECT_PATH)
        .await
        .context("RegisterAuthenticationAgent")?;
    tracing::info!(%session, "registered as PolicyKit authentication agent");

    std::future::pending::<()>().await;
    #[allow(unreachable_code)]
    {
        let _ = authority
            .unregister_authentication_agent(&subject, OBJECT_PATH)
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> Vec<String> {
        vec!["root".to_string(), "1000".to_string()]
    }

    #[test]
    fn resolve_user_falls_back_to_default_on_empty_input() {
        assert_eq!(
            resolve_user("root", &allowed(), ""),
            Some("root".to_string())
        );
    }

    #[test]
    fn resolve_user_accepts_an_offered_identity() {
        assert_eq!(
            resolve_user("root", &allowed(), "1000"),
            Some("1000".to_string())
        );
    }

    #[test]
    fn resolve_user_rejects_a_user_polkit_did_not_offer() {
        assert_eq!(resolve_user("root", &allowed(), "alice"), None);
        assert_eq!(resolve_user("root", &allowed(), "daemon"), None);
    }

    #[test]
    fn resolve_user_is_case_exact() {
        // Usernames are case-significant; "ROOT" is a different principle
        // than the offered "root", so it must be rejected.
        assert_eq!(resolve_user("root", &allowed(), "ROOT"), None);
    }
}

