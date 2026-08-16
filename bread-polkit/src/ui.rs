//! GTK4 password prompt, themed with bread-theme.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gdk::Key;
use gtk4::glib::{self, Propagation};
use gtk4::prelude::*;
use gtk4::{
    Align, Application, ApplicationWindow, Box as GBox, Button, Entry, EventControllerKey, Label,
    Orientation,
};

use bread_theme::tokens;

use crate::agent::UserAction;

const PANEL_WIDTH: i32 = 400;

struct Active {
    cookie: String,
    window: ApplicationWindow,
    password: Entry,
    error: Label,
    reply: tokio::sync::mpsc::Sender<UserAction>,
    username: String,
}

thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
}

/// App-specific rules layered on the shared bread-theme stylesheet.
pub fn app_css() -> String {
    format!(
        ".polkit-panel {{\
             background-color: @surface; color: @on-surface;\
             border-radius: {r}px; padding: {pad}px;\
             min-width: {w}px;\
         }}\n\
         .polkit-title {{ font-size: 1.4em; font-weight: bold; }}\n\
         .polkit-message {{ opacity: 0.85; }}\n\
         .polkit-identity {{ opacity: 0.7; font-size: {sec}px; }}\n\
         .polkit-error {{ color: @on-red; }}\n\
         .polkit-buttons {{ padding-top: {sm}px; }}\n",
        r = tokens::RADIUS_PRIMARY,
        pad = tokens::SPACE_XL,
        w = PANEL_WIDTH,
        sec = tokens::FONT_SIZE_SECONDARY,
        sm = tokens::SPACE_SM,
    )
}

pub struct Prompt {
    pub cookie: String,
    pub message: String,
    pub action_id: String,
    pub username: String,
    pub reply: tokio::sync::mpsc::Sender<UserAction>,
}

/// Show (or replace) the password overlay for this cookie.
pub fn show_prompt(app: &Application, prompt: Prompt) {
    close_if_other_cookie(&prompt.cookie);

    if ACTIVE.with(|a| {
        a.borrow()
            .as_ref()
            .is_some_and(|active| active.cookie == prompt.cookie)
    }) {
        present_existing(&prompt);
        return;
    }

    let window = bread_app::gtk_popup::new_overlay_window(app, "bread-polkit");

    let panel = GBox::new(Orientation::Vertical, tokens::SPACE_MD as i32);
    panel.add_css_class("polkit-panel");
    panel.add_css_class("card");
    panel.set_halign(Align::Center);
    panel.set_valign(Align::Center);
    panel.set_size_request(PANEL_WIDTH, -1);

    let title = Label::new(Some("Authentication required"));
    title.add_css_class("polkit-title");
    title.add_css_class("page-title");
    title.set_halign(Align::Start);
    title.set_wrap(true);
    panel.append(&title);

    let message = if prompt.message.trim().is_empty() {
        prompt.action_id.clone()
    } else {
        prompt.message.clone()
    };
    let msg = Label::new(Some(&message));
    msg.add_css_class("polkit-message");
    msg.set_halign(Align::Start);
    msg.set_wrap(true);
    msg.set_xalign(0.0);
    panel.append(&msg);

    if !prompt.username.is_empty() {
        let identity = Label::new(Some(&format!("Authenticating as {}", prompt.username)));
        identity.add_css_class("polkit-identity");
        identity.add_css_class("dim-label");
        identity.set_halign(Align::Start);
        panel.append(&identity);
    }

    let error = Label::new(None);
    error.add_css_class("polkit-error");
    error.set_halign(Align::Start);
    error.set_wrap(true);
    error.set_visible(false);
    panel.append(&error);

    let password = Entry::builder()
        .visibility(false)
        .input_purpose(gtk4::InputPurpose::Password)
        .placeholder_text("Password")
        .hexpand(true)
        .build();
    panel.append(&password);

    let buttons = GBox::new(Orientation::Horizontal, tokens::SPACE_SM as i32);
    buttons.add_css_class("polkit-buttons");
    buttons.set_halign(Align::End);
    let cancel = Button::with_label("Cancel");
    cancel.add_css_class("flat");
    let confirm = Button::with_label("Authenticate");
    confirm.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&confirm);
    panel.append(&buttons);

    window.set_child(Some(&panel));
    bread_theme::gtk::bind_window_auto_with_app_css(&window, |_| app_css());

    let reply = prompt.reply.clone();
    let cookie = prompt.cookie.clone();
    let username = prompt.username.clone();

    let submit = {
        let password = password.clone();
        let reply = reply.clone();
        let username = username.clone();
        Rc::new(move || {
            let secret = password.text().to_string();
            password.set_text("");
            let _ = reply.try_send(UserAction::Submit {
                username: username.clone(),
                password: secret,
            });
        })
    };
    let cancel_fn = {
        let reply = reply.clone();
        let window = window.clone();
        Rc::new(move || {
            let _ = reply.try_send(UserAction::Cancel);
            window.close();
            ACTIVE.with(|a| a.replace(None));
        })
    };

    confirm.connect_clicked({
        let submit = submit.clone();
        move |_| submit()
    });
    password.connect_activate({
        let submit = submit.clone();
        move |_| submit()
    });
    cancel.connect_clicked({
        let cancel_fn = cancel_fn.clone();
        move |_| cancel_fn()
    });

    let keys = EventControllerKey::new();
    keys.connect_key_pressed({
        let cancel_fn = cancel_fn.clone();
        move |_, key, _, _| {
            if key == Key::Escape {
                cancel_fn();
                Propagation::Stop
            } else {
                Propagation::Proceed
            }
        }
    });
    window.add_controller(keys);

    bread_app::gtk_popup::close_on_outside_click(&window, &panel, {
        let cancel_fn = cancel_fn.clone();
        move || cancel_fn()
    });

    window.connect_close_request({
        let reply = reply.clone();
        move |_| {
            let closing_ours = ACTIVE.with(|a| {
                a.borrow()
                    .as_ref()
                    .is_some_and(|active| active.cookie == cookie)
            });
            if closing_ours {
                let _ = reply.try_send(UserAction::Cancel);
                ACTIVE.with(|a| a.replace(None));
            }
            glib::Propagation::Proceed
        }
    });

    ACTIVE.with(|a| {
        *a.borrow_mut() = Some(Active {
            cookie: prompt.cookie,
            window: window.clone(),
            password: password.clone(),
            error,
            reply,
            username,
        });
    });

    window.present();
    password.grab_focus();
}

fn present_existing(prompt: &Prompt) {
    ACTIVE.with(|a| {
        if let Some(active) = a.borrow_mut().as_mut() {
            active.reply = prompt.reply.clone();
            active.username = prompt.username.clone();
            active.error.set_visible(false);
            active.password.set_text("");
            active.window.present();
            active.password.grab_focus();
        }
    });
}

/// Show a retry message on the open dialog for `cookie`.
pub fn show_retry(cookie: &str, message: &str) {
    ACTIVE.with(|a| {
        let mut guard = a.borrow_mut();
        let Some(active) = guard.as_mut() else {
            return;
        };
        if active.cookie != cookie {
            return;
        }
        active.error.set_label(message);
        active.error.set_visible(true);
        active.password.set_text("");
        active.window.present();
        active.password.grab_focus();
    });
}

/// Close the dialog if it is still showing `cookie`.
pub fn close_prompt(cookie: &str) {
    ACTIVE.with(|a| {
        let Some(active) = a.borrow_mut().take() else {
            return;
        };
        if active.cookie == cookie {
            active.window.close();
        } else {
            *a.borrow_mut() = Some(active);
        }
    });
}

fn close_if_other_cookie(cookie: &str) {
    ACTIVE.with(|a| {
        let Some(active) = a.borrow_mut().take() else {
            return;
        };
        if active.cookie == cookie {
            *a.borrow_mut() = Some(active);
        } else {
            active.window.close();
        }
    });
}
