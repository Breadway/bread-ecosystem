//! `bread-theme` — generates the ecosystem's shared GTK stylesheet from the
//! current pywal palette and writes it to the canonical path that every bread
//! GUI loads. Run it at session start, and again after the wallpaper/palette
//! changes (e.g. from a pywal hook); apps watch the file and recolour live.
//!
//!   bread-theme            # same as `generate`
//!   bread-theme generate   # render + write the shared stylesheet
//!   bread-theme reload      # re-render from the current pywal palette and
//!                           # signal every running bread GUI to recolour
//!   bread-theme path       # print the stylesheet path
//!   bread-theme print      # render to stdout (no write)

use std::process::ExitCode;

fn write_and_report(verb: &str) -> ExitCode {
    match bread_theme::write_shared_css() {
        Ok(path) => {
            eprintln!("bread-theme: {verb} {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("bread-theme: failed to write stylesheet: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| "generate".into());
    match cmd.as_str() {
        "path" => {
            println!("{}", bread_theme::shared_css_path().display());
            ExitCode::SUCCESS
        }
        "print" => {
            print!("{}", bread_theme::render());
            ExitCode::SUCCESS
        }
        "generate" => write_and_report("wrote"),
        // `reload` is `generate` from the caller's view, but it's the verb to use
        // after changing pywal colours: rewriting the file (atomic rename) trips
        // the file monitor in every running bread GUI, so they all re-read the
        // palette and recolour live — shared widgets *and* each app's own rules.
        "reload" => write_and_report("reloaded"),
        "-h" | "--help" | "help" => {
            eprintln!(
                "bread-theme — shared stylesheet generator\n\n\
                 USAGE:\n  bread-theme [generate|reload|path|print]\n\n\
                 generate  render the pywal palette to the shared stylesheet (default)\n\
                 reload    re-render and signal running bread GUIs to recolour live\n\
                 path      print the stylesheet path ({})\n\
                 print     render to stdout without writing",
                bread_theme::shared_css_path().display()
            );
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("bread-theme: unknown command '{other}' (try generate|reload|path|print)");
            ExitCode::FAILURE
        }
    }
}
