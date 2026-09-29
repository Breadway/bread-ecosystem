//! Print the native extractor's pywal colour list for each image argument,
//! one JSON object per line: `{"path": ..., "colors": [...] | null,
//! "error": ... , "ms": ...}`. Used by `scripts/compare-pywal.py`; not part
//! of any runtime path.

use std::time::Instant;

fn main() {
    for arg in std::env::args().skip(1) {
        let start = Instant::now();
        let result = bread_palette_extract::wal_colors(std::path::Path::new(&arg));
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let line = match result {
            Ok(colors) => serde_json::json!({ "path": arg, "colors": colors, "ms": ms }),
            Err(e) => serde_json::json!({ "path": arg, "error": e.to_string(), "ms": ms }),
        };
        println!("{line}");
    }
}
