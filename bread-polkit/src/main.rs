//! bread-polkit — themed PolicyKit authentication agent.
//!
//! Registers on the `org.freedesktop.PolicyKit1.AuthenticationAgent`
//! interface and shows a bread-theme GTK4 password prompt. This is an
//! agent, not a wrapper that execs `polkit-gnome`.
//!
//! Autostart: copy `contrib/bread-polkit.desktop` to
//! `~/.config/autostart/`, or add `exec-once = bread-polkit` to Hyprland.

mod agent;
mod auth;
mod ui;

use bread_app::singleton::Acquire;
use gtk4::prelude::*;

const APP_NAME: &str = "bread-polkit";

fn main() {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("-h") | Some("--help") => {
            print_help();
            return;
        }
        Some("-V") | Some("--version") => {
            println!("bread-polkit {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some(other) => {
            eprintln!("bread-polkit: unknown argument '{other}'");
            print_help();
            std::process::exit(2);
        }
        None => {}
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let _guard = match bread_app::try_acquire(APP_NAME) {
        Ok(Acquire::Acquired(g)) => Some(g),
        Ok(Acquire::HeldByOther(pid)) => {
            eprintln!("bread-polkit: already running (pid {pid:?})");
            std::process::exit(0);
        }
        Err(e) => {
            // Don't keep running without the single-instance lock: a second
            // copy would attempt to `serve_at` the same PolicyKit agent
            // object path on the system bus, and a password prompt held by a
            // process whose lock couldn't be taken is ambiguous state. Fail
            // fast and let a wrapper/autostart retry.
            eprintln!("bread-polkit: singleton lock unavailable ({e}); exiting");
            std::process::exit(1);
        }
    };

    let app_id = bread_app::application_id(APP_NAME).expect("static app name");
    let app = gtk4::Application::builder().application_id(&app_id).build();

    app.connect_activate(|app| {
        bread_theme::gtk::apply_shared();
        bread_theme::gtk::apply_app_css(ui::app_css);
        // No window until polkit asks; hold so GApplication stays alive.
        std::mem::forget(app.hold());
        if let Err(e) = agent::spawn() {
            eprintln!("bread-polkit: {e:#}");
            app.quit();
        }
    });

    app.run();
}

fn print_help() {
    print!(
        "\
bread-polkit — themed PolicyKit authentication agent

Usage:
  bread-polkit
  bread-polkit --help
  bread-polkit --version

Autostart (pick one):
  cp contrib/bread-polkit.desktop ~/.config/autostart/
  exec-once = bread-polkit          # Hyprland

The agent talks to the polkit1 AuthenticationAgent API and prompts for
a password. It does not exec polkit-gnome. Not a bakery product; not
on the BOS ISO lockfile.
"
    );
}
