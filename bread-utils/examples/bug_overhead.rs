//! Measures what breadbug costs an app on its hot paths: `BugGuard::tick`
//! (called from the main loop), `BugGuard::log` (called per log line), and the
//! `log::Log` backend. Run in release mode:
//!
//! ```text
//! cargo run --release -p bread-utils --features bread-client --example bug_overhead
//! ```
//!
//! Each case runs `ITER` calls and reports nanoseconds per call. Log cases run
//! against a socket nothing reads (breadd absent): the caller must not wait,
//! and the number is the cost of queueing a line, not of sending it.

use std::hint::black_box;
use std::time::{Duration, Instant};

use bread_utils::bread_client::BreadClient;
use bread_utils::bug::BugOptions;

const ITER: u64 = 1_000_000;

fn per_call(label: &str, total: Duration, n: u64) {
    let ns = total.as_nanos() as f64 / n as f64;
    println!(
        "{label:<44} {ns:>10.1} ns/call   ({n} calls, {:.1} ms total)",
        total.as_secs_f64() * 1e3
    );
}

fn main() {
    let absent = std::env::temp_dir().join(format!("bug-overhead-{}.sock", std::process::id()));

    let mut opts = BugOptions::disabled();
    opts.heartbeat = true;
    opts.heartbeat_interval = Duration::from_secs(2);
    opts.forward_logs = true;
    opts.socket = Some(absent.clone());
    let client = BreadClient::connect_without_bug_reporting("bench");
    let guard = client.enable_bug_reporting(opts);

    // tick: an atomic store of a millisecond timestamp.
    let start = Instant::now();
    for _ in 0..ITER {
        guard.tick();
    }
    per_call("tick()", start.elapsed(), ITER);

    // log: format-free queue push, lines dropped oldest-first once full.
    let start = Instant::now();
    for _ in 0..ITER / 10 {
        guard.log("info", black_box("a typical log line of about forty bytes"));
    }
    per_call(
        "log() (breadd absent, queue full)",
        start.elapsed(),
        ITER / 10,
    );

    // log::Log backend: the same path via the `log` crate.
    use log::Log;
    let logger = guard.logger();
    let record = log::Record::builder()
        .args(format_args!("a typical log line of about forty bytes"))
        .level(log::Level::Info)
        .target("bench")
        .build();
    let start = Instant::now();
    for _ in 0..ITER / 10 {
        logger.log(black_box(&record));
    }
    per_call(
        "log::Log::log (breadd absent, queue full)",
        start.elapsed(),
        ITER / 10,
    );

    println!("dropped lines: {}", guard.dropped_logs());
}
