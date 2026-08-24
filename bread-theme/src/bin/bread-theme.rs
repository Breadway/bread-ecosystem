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
//!   bread-theme generate-output <OUTPUT> --image <PATH> [--shared]
//!   bread-theme generate-output <OUTPUT> --from-json <PATH> [--shared]
//!   bread-theme layerrules  # write the active theme's [compositor] table
//!                           # to ~/.config/hypr/layerrules.json (plan §9)

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

fn print_help() {
    eprintln!(
        "bread-theme — shared stylesheet generator\n\n\
         USAGE:\n\
         \x20 bread-theme [generate|reload|path|print|layerrules]\n\
         \x20 bread-theme generate-output <OUTPUT> --image <PATH> [--shared]\n\
         \x20 bread-theme generate-output <OUTPUT> --from-json <WAL-OR-PALETTE.json> [--shared]\n\n\
         generate          render the pywal palette to the shared stylesheet (default)\n\
         reload            re-render and signal running bread GUIs to recolour live\n\
         path              print the stylesheet path ({})\n\
         print             render to stdout without writing\n\
         generate-output   write palettes/<OUTPUT>.json and themes/<OUTPUT>.css\n\
         \x20                --image      isolated `wal -i` (does not touch ~/.cache/wal)\n\
         \x20                --from-json  wal colors.json or a color1-6 object\n\
         \x20                --shared     also write the session-global theme.css\n\
         layerrules        write the active shell theme's [compositor] table to\n\
         \x20                {} —\n\
         \x20                scripts/ui/rules.lua reads it for per-namespace blur/\n\
         \x20                animation, falling back to its hardcoded rules if this\n\
         \x20                is missing or malformed",
        bread_theme::shared_css_path().display(),
        bread_theme::layerrules_path().display()
    );
}

fn generate_output_cmd() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(2).collect();
    if args.is_empty()
        || args
            .iter()
            .any(|a| matches!(a.as_str(), "-h" | "--help" | "help"))
    {
        print_help();
        return if args.is_empty() {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        };
    }

    let output = args[0].as_str();
    if output.starts_with('-') {
        eprintln!("bread-theme: generate-output requires an OUTPUT name (got '{output}')");
        return ExitCode::FAILURE;
    }

    let mut image: Option<&str> = None;
    let mut from_json: Option<&str> = None;
    let mut shared = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--shared" => shared = true,
            "--image" => {
                i += 1;
                match args.get(i) {
                    Some(p) => image = Some(p.as_str()),
                    None => {
                        eprintln!("bread-theme: --image requires a path");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--from-json" => {
                i += 1;
                match args.get(i) {
                    Some(p) => from_json = Some(p.as_str()),
                    None => {
                        eprintln!("bread-theme: --from-json requires a path");
                        return ExitCode::FAILURE;
                    }
                }
            }
            other => {
                eprintln!("bread-theme: unknown generate-output flag '{other}'");
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }

    match (image, from_json) {
        (Some(_), Some(_)) => {
            eprintln!("bread-theme: pass only one of --image or --from-json");
            ExitCode::FAILURE
        }
        (None, None) => {
            eprintln!("bread-theme: generate-output needs --image <PATH> or --from-json <PATH>");
            ExitCode::FAILURE
        }
        (Some(path), None) => {
            match bread_theme::generate_output(output, std::path::Path::new(path)) {
                Ok(css) => finish_generate_output(output, css, shared),
                Err(e) => {
                    eprintln!("bread-theme: generate-output failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        (None, Some(path)) => match write_output_from_json(output, path, shared) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bread-theme: generate-output failed: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

fn write_output_from_json(output: &str, json_path: &str, shared: bool) -> std::io::Result<()> {
    let json = std::fs::read_to_string(json_path)?;
    let palette = bread_theme::palette_from_json(&json).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("could not parse palette JSON: {json_path}"),
        )
    })?;
    let pal_path = bread_theme::write_output_palette(output, &palette)?;
    let css_path = bread_theme::write_output_css(output, &palette)?;
    eprintln!(
        "bread-theme: wrote {} and {}",
        pal_path.display(),
        css_path.display()
    );
    if shared {
        let shared_path = bread_theme::write_shared_css_from(&palette)?;
        eprintln!("bread-theme: wrote shared {}", shared_path.display());
    }
    Ok(())
}

fn finish_generate_output(output: &str, css: std::path::PathBuf, shared: bool) -> ExitCode {
    eprintln!("bread-theme: wrote {}", css.display());
    if shared {
        match bread_theme::write_shared_css_from(&bread_theme::load_palette_for(output)) {
            Ok(path) => {
                eprintln!("bread-theme: wrote shared {}", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("bread-theme: failed to write shared stylesheet: {e}");
                ExitCode::FAILURE
            }
        }
    } else {
        ExitCode::SUCCESS
    }
}

fn layerrules_cmd() -> ExitCode {
    match bread_theme::write_layerrules_active() {
        Ok(path) => {
            eprintln!("bread-theme: wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("bread-theme: failed to write layer rules: {e}");
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
        "generate-output" => generate_output_cmd(),
        "layerrules" => layerrules_cmd(),
        "-h" | "--help" | "help" => {
            print_help();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!(
                "bread-theme: unknown command '{other}' (try generate|reload|path|print|generate-output|layerrules)"
            );
            ExitCode::FAILURE
        }
    }
}
