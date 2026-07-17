//! Non-destructive TOML config editing discipline.
//!
//! Extracted from `bos-settings/src/config/mod.rs` (`load_doc`/`save_doc`)
//! and `breadhelp/src/config.rs`, which re-implemented the exact same
//! function bodies in the same fix pass that introduced `bos-settings`'s
//! version — right down to the eprintln wording template. Both parse into a
//! `toml_edit::DocumentMut` (preserving keys/comments/formatting this app
//! doesn't model) and back up a file that exists but fails to parse, once,
//! before falling back to an empty document — so a bad edit is always
//! recoverable from `<path>.bak` instead of silently destroying whatever the
//! file used to hold.
//!
//! Requires the `toml` feature.

use std::path::Path;
use toml_edit::DocumentMut;

/// Load a TOML file into an editable document. A missing file yields an
/// empty document (normal for a fresh install). A file that *exists* but
/// fails to parse is backed up to `<path>.bak` once before falling back to
/// an empty document, so the next [`save_doc`] doesn't silently overwrite an
/// unparseable-but-recoverable file with only the caller's modelled keys.
///
/// `app` is used only to prefix the parse-failure log line (e.g.
/// `"breadhelp"`, `"bos-settings"`).
pub fn load_doc(app: &str, path: &Path) -> DocumentMut {
    let Ok(text) = std::fs::read_to_string(path) else {
        return DocumentMut::default();
    };
    match text.parse::<DocumentMut>() {
        Ok(doc) => doc,
        Err(e) => {
            let backup = super::atomic::backup_path_for(path);
            eprintln!(
                "{app}: {} failed to parse ({e}); backed up to {} before falling back to defaults",
                path.display(),
                backup.display()
            );
            let _ = std::fs::write(&backup, &text);
            DocumentMut::default()
        }
    }
}

/// Write the document back to disk atomically (temp-then-rename), backing up
/// whatever was there before overwriting it — see
/// [`crate::atomic::write_atomic_backed_up`].
pub fn save_doc(path: &Path, doc: &DocumentMut) -> std::io::Result<()> {
    super::atomic::write_atomic_backed_up(path, &doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml_edit::value;

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bread-utils-tomlcfg-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_yields_empty_document() {
        let dir = tmp_dir("missing");
        let doc = load_doc("test", &dir.join("nope.toml"));
        assert!(doc.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tmp_dir("roundtrip");
        let path = dir.join("state.toml");
        let mut doc = DocumentMut::default();
        doc["general"]["mode"] = value("dad");
        save_doc(&path, &doc).unwrap();

        let loaded = load_doc("test", &path);
        assert_eq!(
            loaded.get("general").and_then(|t| t.get("mode")).and_then(|v| v.as_str()),
            Some("dad")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unparseable_existing_file_is_backed_up_before_falling_back() {
        let dir = tmp_dir("bad-parse");
        let path = dir.join("state.toml");
        std::fs::write(&path, "this is not [ valid toml").unwrap();

        let doc = load_doc("test", &path);
        assert!(doc.is_empty());
        let backup = dir.join("state.toml.bak");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "this is not [ valid toml");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
