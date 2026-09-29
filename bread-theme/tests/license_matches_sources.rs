//! `bread-theme/LICENSE` is a hand-maintained combination of bread-theme's
//! own MIT text and bread-palette-extract's ImageMagick and libjpeg-turbo
//! (IJG) licence texts, kept as one file because bakery ships only the
//! compiled binary: `bakery.toml`'s `license_file` can only point at a
//! single file, and the release workflows copy that one file into the
//! package, so it is the only licence text an end user of the `extract`
//! feature actually receives. Nothing about `include_str!` or the build
//! enforces that this file stays in sync with its sources; this test
//! catches the drift the moment `LICENSE-ImageMagick` or
//! `LICENSE-libjpeg-turbo` changes and `bread-theme/LICENSE` is not
//! regenerated to match.

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"))
}

#[test]
fn license_starts_with_bread_themes_own_mit_text() {
    let combined = read("LICENSE");
    assert!(
        combined.starts_with("MIT License"),
        "bread-theme/LICENSE must lead with bread-theme's own MIT licence"
    );
    assert!(
        combined.contains("THE SOFTWARE IS PROVIDED \"AS IS\""),
        "bread-theme/LICENSE is missing the standard MIT disclaimer text"
    );
}

#[test]
fn license_carries_the_imagemagick_license_verbatim() {
    let combined = read("LICENSE");
    let source = read("../bread-palette-extract/LICENSE-ImageMagick");
    assert!(
        combined.contains(source.trim()),
        "bread-theme/LICENSE has drifted from bread-palette-extract/LICENSE-ImageMagick: \
         regenerate the combined LICENSE from its sources"
    );
}

#[test]
fn license_carries_the_libjpeg_turbo_license_verbatim() {
    let combined = read("LICENSE");
    let source = read("../bread-palette-extract/LICENSE-libjpeg-turbo");
    assert!(
        combined.contains(source.trim()),
        "bread-theme/LICENSE has drifted from bread-palette-extract/LICENSE-libjpeg-turbo: \
         regenerate the combined LICENSE from its sources"
    );
    // Clause 2 of the IJG License: an executable-only distribution's
    // documentation must carry this exact sentence. It only reaches the
    // user through this combined file (and `bread-theme notice`), so pin
    // it directly rather than trusting the substring check above alone.
    assert!(
        combined.contains("This software is based in part on the work of the Independent JPEG"),
        "bread-theme/LICENSE is missing the IJG attribution sentence required for an \
         executable-only distribution"
    );
}
