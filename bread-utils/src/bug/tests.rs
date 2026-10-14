use super::*;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;

/// A socket that records every request line it receives.
struct Recorder {
    path: PathBuf,
    lines: Arc<Mutex<Vec<Value>>>,
    _dir: tempfile::TempDir,
}

fn recorder() -> Recorder {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("breadd.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let l2 = lines.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let l3 = l2.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                        l3.lock().unwrap().push(v);
                    }
                }
            });
        }
    });
    Recorder {
        path,
        lines,
        _dir: dir,
    }
}

impl Recorder {
    fn methods(&self, m: &str) -> Vec<Value> {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .filter(|v| v["method"] == m)
            .cloned()
            .collect()
    }

    fn wait_for(&self, m: &str, n: usize) -> Vec<Value> {
        for _ in 0..200 {
            let got = self.methods(m);
            if got.len() >= n {
                return got;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        self.methods(m)
    }

    fn all_log_lines(&self) -> Vec<Value> {
        self.methods("bug.logs")
            .iter()
            .flat_map(|c| c["params"]["lines"].as_array().unwrap().clone())
            .collect()
    }
}

fn opts(path: &Path) -> BugOptions {
    let mut o = BugOptions::new("0.3.1");
    o.panic_hook = false;
    o.native_crashes = false;
    o.heartbeat_interval = Duration::from_millis(250);
    o.socket = Some(path.to_path_buf());
    o
}

#[test]
fn ticks_gate_heartbeats_so_a_frozen_loop_goes_quiet() {
    let r = recorder();
    let client = BreadClient::connect_without_bug_reporting("clip");
    let guard = client.enable_bug_reporting(opts(&r.path));
    // never ticked: nothing is sent, even after several intervals
    std::thread::sleep(Duration::from_millis(700));
    assert!(r.methods("bug.heartbeat").is_empty());

    // ticking: heartbeats flow with pid and interval
    for _ in 0..12 {
        guard.tick();
        std::thread::sleep(Duration::from_millis(100));
    }
    let beats = r.wait_for("bug.heartbeat", 2);
    assert!(beats.len() >= 2, "{beats:?}");
    assert_eq!(beats[0]["params"]["app"], "clip");
    assert_eq!(beats[0]["params"]["pid"], std::process::id());
    assert_eq!(beats[0]["params"]["interval_ms"], 250);

    // loop freezes: no new beats after the grace of two intervals
    std::thread::sleep(Duration::from_millis(700));
    let before = r.methods("bug.heartbeat").len();
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(r.methods("bug.heartbeat").len(), before);
}

#[test]
fn unconditional_heartbeats_and_clean_stop_on_drop() {
    let r = recorder();
    let client = BreadClient::connect_without_bug_reporting("pad");
    let mut o = opts(&r.path);
    o.heartbeat_unconditional = true;
    let guard = client.enable_bug_reporting(o);
    assert!(!r.wait_for("bug.heartbeat", 1).is_empty());
    drop(guard);
    let beats = r.wait_for("bug.heartbeat", 2);
    let last = beats.last().unwrap();
    assert_eq!(last["params"]["stopping"], true);
    assert_eq!(last["params"]["app"], "pad");
}

#[test]
fn logs_are_batched_clipped_and_leveled() {
    let r = recorder();
    let client = BreadClient::connect_without_bug_reporting("clip");
    let mut o = opts(&r.path);
    o.heartbeat = false;
    let guard = client.enable_bug_reporting(o);
    guard.log("warning", "first");
    guard.log("bogus-level", &"x".repeat(10_000));
    guard.log("error", "third");
    r.wait_for("bug.logs", 1);
    let mut lines = r.all_log_lines();
    for _ in 0..40 {
        if lines.len() >= 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        lines = r.all_log_lines();
    }
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(
        (lines[0]["level"].as_str(), lines[0]["msg"].as_str()),
        (Some("WARN"), Some("first"))
    );
    assert_eq!(lines[1]["level"], "INFO");
    assert!(lines[1]["msg"].as_str().unwrap().len() <= MAX_LOG_BYTES);
    assert_eq!(lines[2]["level"], "ERROR");
    assert!(lines[0]["t"].as_u64().unwrap() > 0);
}

#[test]
fn a_flood_never_blocks_the_caller_and_overflow_is_counted() {
    // No server at all: lines pile up, the caller must still return promptly.
    let dir = tempfile::tempdir().unwrap();
    let client = BreadClient::connect_without_bug_reporting("clip");
    let mut o = opts(&dir.path().join("nobody.sock"));
    o.heartbeat = false;
    let guard = client.enable_bug_reporting(o);
    let start = Instant::now();
    for i in 0..20_000 {
        guard.log("info", &format!("line {i}"));
    }
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "logging blocked the caller"
    );
    assert!(guard.dropped_logs() > 0);
}

#[test]
fn overflow_drops_the_oldest_lines_and_keeps_the_newest() {
    let q = Queue::new();
    for i in 0..(LOG_QUEUE + 10) {
        q.push(Line {
            level: "INFO",
            msg: format!("{i}"),
            t: 0,
        });
    }
    let kept: Vec<String> = q
        .take(LOG_QUEUE + 10, Duration::ZERO)
        .into_iter()
        .map(|l| l.msg)
        .collect();
    assert_eq!(kept.len(), LOG_QUEUE);
    assert_eq!(kept.first().map(String::as_str), Some("10"));
    assert_eq!(
        kept.last().map(String::as_str),
        Some((LOG_QUEUE + 9).to_string().as_str())
    );
}

#[test]
fn a_sustained_flood_is_sent_at_the_rate_budget_not_all_at_once() {
    let r = recorder();
    let client = BreadClient::connect_without_bug_reporting("clip");
    let mut o = opts(&r.path);
    o.heartbeat = false;
    let guard = client.enable_bug_reporting(o);
    // Ten times the burst, all at once: the queue keeps only what fits, and
    // the sender's budget keeps the total far below the flood size.
    let start = Instant::now();
    for i in 0..(LOG_BURST as usize * 10) {
        guard.log("info", &format!("flood {i}"));
    }
    std::thread::sleep(Duration::from_millis(1200));
    let sent = r.all_log_lines().len() as u64;
    // Burst plus the sustained rate over the window, plus one batch of slack
    // for the final flush and timer granularity.
    let window = start.elapsed().as_secs_f64();
    let budget = LOG_BURST + (LOG_RATE_PER_SEC as f64 * window).ceil() as u64 + LOG_BATCH as u64;
    assert!(sent <= budget, "sent {sent}, budget {budget}");
    assert!(sent > 0);
}

#[test]
fn shutdown_is_bounded_even_while_a_logger_clone_is_alive() {
    // Regression: a cloned logger kept the channel open, so dropping the guard
    // used to wait on the log thread forever (3s+ in practice).
    let dir = tempfile::tempdir().unwrap();
    let client = BreadClient::connect_without_bug_reporting("clip");
    let mut o = opts(&dir.path().join("nobody.sock"));
    o.heartbeat = true;
    let guard = client.enable_bug_reporting(o);
    let logger = guard.logger();
    let start = Instant::now();
    drop(guard);
    let took = start.elapsed();
    assert!(took < Duration::from_secs(1), "drop took {took:?}");
    // The clone outlives the guard and must be harmless now.
    use log::Log;
    logger.log(
        &log::Record::builder()
            .args(format_args!("after shutdown"))
            .level(log::Level::Warn)
            .target("clip")
            .build(),
    );
}

#[test]
fn log_backend_forwards_records() {
    use log::Log;
    let r = recorder();
    let client = BreadClient::connect_without_bug_reporting("clip");
    let mut o = opts(&r.path);
    o.heartbeat = false;
    let guard = client.enable_bug_reporting(o);
    let logger = guard.logger();
    logger.log(
        &log::Record::builder()
            .args(format_args!("hello {}", 42))
            .level(log::Level::Warn)
            .target("clip::sync")
            .build(),
    );
    let calls = r.wait_for("bug.logs", 1);
    let l = &calls[0]["params"]["lines"][0];
    assert_eq!(l["level"], "WARN");
    assert_eq!(l["msg"], "clip::sync: hello 42");
}

#[test]
fn forwarded_logs_are_redacted_before_they_reach_the_socket() {
    let r = recorder();
    let client = BreadClient::connect_without_bug_reporting("clip");
    let mut o = opts(&r.path);
    o.heartbeat = false;
    let guard = client.enable_bug_reporting(o);
    guard.log(
        "info",
        "login for dana.fake@example.com with token sk-FAKE0123456789ABCDEF from 10.1.2.3",
    );
    r.wait_for("bug.logs", 1);
    let wire = serde_json::to_string(&r.all_log_lines()).unwrap();
    for leak in [
        "dana.fake@example.com",
        "sk-FAKE0123456789ABCDEF",
        "10.1.2.3",
    ] {
        assert!(!wire.contains(leak), "{leak} leaked: {wire}");
    }
    assert!(wire.contains("<email>"));
}

#[test]
fn disabled_options_start_nothing_and_missing_breadd_is_harmless() {
    let client = BreadClient::connect_without_bug_reporting("clip");
    let guard = client.enable_bug_reporting(BugOptions::disabled());
    guard.log("info", "dropped silently");
    guard.tick();
    assert_eq!(guard.dropped_logs(), 0);
    drop(guard);

    let dir = tempfile::tempdir().unwrap();
    let mut o = opts(&dir.path().join("none.sock"));
    o.heartbeat_unconditional = true;
    let g = BreadClient::connect_without_bug_reporting("clip").enable_bug_reporting(o);
    std::thread::sleep(Duration::from_millis(300));
    drop(g); // must not hang or panic
}

#[test]
fn opt_out_values_are_recognised() {
    assert!(automatic_allowed(None));
    assert!(automatic_allowed(Some("1")));
    assert!(!automatic_allowed(Some("0")));
    assert!(!automatic_allowed(Some(" OFF ")));
    assert!(!automatic_allowed(Some("false")));
}

#[test]
fn level_names() {
    assert_eq!(level_name("warning"), "WARN");
    assert_eq!(level_name("fatal"), "ERROR");
    assert_eq!(level_name("???"), "INFO");
}
