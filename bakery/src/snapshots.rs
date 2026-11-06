//! Optional Snapper pairs around a whole package command, including failures.
use crate::prefix::Layout;
use serde::Deserialize;
use std::io;
use std::path::Path;
use std::process::{Command, Output, Stdio};

type Runner = Box<dyn FnMut(&[String]) -> io::Result<Output>>;

#[derive(Default, Deserialize)]
struct Config {
    #[serde(default)]
    snapper_configs: Vec<String>,
}

pub struct Transaction {
    pairs: Vec<(String, u64)>,
    operation: String,
    run: Runner,
}

pub fn enabled(layout: &Layout) -> bool {
    !selected_configs(
        layout,
        Some("update"),
        false,
        Path::new("/etc/bakery/config.toml"),
    )
    .is_empty()
}

impl Transaction {
    pub fn begin(layout: &Layout, operation: Option<&str>, dry_run: bool) -> Self {
        let configs = selected_configs(
            layout,
            operation,
            dry_run,
            Path::new("/etc/bakery/config.toml"),
        );
        Self::start(
            configs,
            operation.unwrap_or("command"),
            Box::new(run_snapper),
        )
    }

    fn start(configs: Vec<String>, operation: &str, mut run: Runner) -> Self {
        let mut pairs = Vec::new();
        for config in configs {
            let args = vec![
                "-c".into(),
                config.clone(),
                "create".into(),
                "-t".into(),
                "pre".into(),
                "-p".into(),
                "-c".into(),
                "number".into(),
                "-d".into(),
                format!("bakery {operation} (pre)"),
            ];
            match run(&args) {
                Ok(output) if output.status.success() => {
                    match String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .parse::<u64>()
                    {
                        Ok(number) if number > 0 => pairs.push((config, number)),
                        _ => warn(&config, "pre snapshot did not return a valid number"),
                    }
                }
                Ok(output) => warn(&config, &String::from_utf8_lossy(&output.stderr)),
                Err(error) => warn(&config, &error.to_string()),
            }
        }
        Self {
            pairs,
            operation: operation.into(),
            run,
        }
    }

    pub fn finish(&mut self, success: bool) {
        let outcome = if success { "success" } else { "failed" };
        for (config, number) in self.pairs.drain(..) {
            let args = vec![
                "-c".into(),
                config.clone(),
                "create".into(),
                "-t".into(),
                "post".into(),
                "--pre-number".into(),
                number.to_string(),
                "-c".into(),
                "number".into(),
                "-d".into(),
                format!("bakery {} (post, {outcome})", self.operation),
            ];
            match (self.run)(&args) {
                Ok(output) if output.status.success() => {}
                Ok(output) => warn(&config, &String::from_utf8_lossy(&output.stderr)),
                Err(error) => warn(&config, &error.to_string()),
            }
        }
    }
}

fn selected_configs(
    layout: &Layout,
    operation: Option<&str>,
    dry_run: bool,
    path: &Path,
) -> Vec<String> {
    if layout.is_system && !dry_run && operation.is_some() {
        configured(path)
    } else {
        Vec::new()
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        // Also close pairs if a caller unwinds before explicit completion.
        self.finish(false);
    }
}

fn configured(path: &Path) -> Vec<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            warn("config", &error.to_string());
            return Vec::new();
        }
    };
    let config: Config = match toml::from_str(&text) {
        Ok(config) => config,
        Err(error) => {
            warn("config", &error.to_string());
            return Vec::new();
        }
    };
    let mut names = Vec::new();
    for name in config.snapper_configs {
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            warn("config", "invalid Snapper config name, ignoring it");
        } else if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn run_snapper(args: &[String]) -> io::Result<Output> {
    let output = Command::new("/usr/bin/snapper")
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    if output.status.success() {
        return Ok(output);
    }
    // No interactive password prompts, including from GUI package updates.
    match Command::new("/usr/bin/sudo")
        .args(["-n", "/usr/bin/snapper"])
        .args(args)
        .stdin(Stdio::null())
        .output()
    {
        Ok(privileged) if privileged.status.success() => Ok(privileged),
        _ => Ok(output),
    }
}

fn warn(config: &str, reason: &str) {
    eprintln!("  warning: bakery Snapper {config}: {}", reason.trim());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::process::ExitStatusExt;
    use std::rc::Rc;

    fn output(number: &str, success: bool) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(if success { 0 } else { 256 }),
            stdout: number.as_bytes().to_vec(),
            stderr: b"unavailable".to_vec(),
        }
    }

    #[test]
    fn pairs_match_for_success_and_failure_without_duplicate_drop_posts() {
        for success in [true, false] {
            let calls = Rc::new(RefCell::new(Vec::new()));
            let log = calls.clone();
            let mut transaction = Transaction::start(
                vec!["root".into(), "home".into()],
                "update",
                Box::new(move |args| {
                    log.borrow_mut().push(args.to_vec());
                    Ok(output(
                        if args[1] == "root" { "41\n" } else { "72\n" },
                        true,
                    ))
                }),
            );
            transaction.finish(success);
            drop(transaction);
            let calls = calls.borrow();
            assert_eq!(calls.len(), 4);
            assert_eq!(&calls[2][5..7], &["--pre-number", "41"]);
            assert_eq!(&calls[3][5..7], &["--pre-number", "72"]);
            assert!(calls[2]
                .last()
                .unwrap()
                .contains(if success { "success" } else { "failed" }));
        }
    }

    #[test]
    fn failed_or_invalid_pre_does_not_produce_unmatched_post() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let log = calls.clone();
        let transaction = Transaction::start(
            vec!["root".into(), "home".into()],
            "install",
            Box::new(move |args| {
                log.borrow_mut().push(args.to_vec());
                Ok(output("not a number", args[1] == "root"))
            }),
        );
        drop(transaction);
        assert_eq!(calls.borrow().len(), 2);
    }

    #[test]
    fn drop_closes_a_partial_failed_command() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let log = calls.clone();
        drop(Transaction::start(
            vec!["root".into()],
            "remove",
            Box::new(move |args| {
                log.borrow_mut().push(args.to_vec());
                Ok(output("9", true))
            }),
        ));
        assert_eq!(calls.borrow().len(), 2);
        assert!(calls.borrow()[1].last().unwrap().contains("failed"));
    }

    #[test]
    fn config_opt_in_deduplicates_and_rejects_unsafe_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(configured(&path).is_empty());
        std::fs::write(&path, "prefix = '/usr/local'").unwrap();
        assert!(configured(&path).is_empty());
        std::fs::write(
            &path,
            "snapper_configs = ['root', 'home', 'root', '../escape', '']",
        )
        .unwrap();
        assert_eq!(configured(&path), vec!["root", "home"]);
    }

    #[test]
    fn user_read_only_and_dry_run_skip_snapshot_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "snapper_configs = ['root']").unwrap();
        let system = Layout::from_prefix(Path::new("/usr/local"), None);
        let user = Layout::user_default(None);
        assert!(selected_configs(&system, Some("update"), true, &path).is_empty());
        assert!(selected_configs(&system, None, false, &path).is_empty());
        assert!(selected_configs(&user, Some("update"), false, &path).is_empty());
        assert_eq!(
            selected_configs(&system, Some("update"), false, &path),
            vec!["root"]
        );
    }
}
