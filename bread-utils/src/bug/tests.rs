use super::*;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::sync::Mutex;

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
    let client = BreadClient::connect("clip");
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
    let client = BreadClient::connect("pad");
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
    let client = BreadClient::connect("clip");
    let mut o = opts(&r.path);
    o.heartbeat = false;
    let guard = client.enable_bug_reporting(o);
    guard.log("warning", "first");
    guard.log("bogus-level", &"x".repeat(10_000));
    guard.log("error", "third");
    let calls = r.wait_for("bug.logs", 1);
    let mut lines = Vec::new();
    for c in &calls {
        lines.extend(c["params"]["lines"].as_array().unwrap().clone());
    }
    for _ in 0..40 {
        if lines.len() >= 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        lines.clear();
        for c in r.methods("bug.logs") {
            lines.extend(c["params"]["lines"].as_array().unwrap().clone());
        }
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
    let client = BreadClient::connect("clip");
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
fn log_backend_forwards_records() {
    use log::Log;
    let r = recorder();
    let client = BreadClient::connect("clip");
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
fn disabled_options_start_nothing_and_missing_breadd_is_harmless() {
    let client = BreadClient::connect("clip");
    let guard = client.enable_bug_reporting(BugOptions::disabled());
    guard.log("info", "dropped silently");
    guard.tick();
    assert_eq!(guard.dropped_logs(), 0);
    drop(guard);

    let dir = tempfile::tempdir().unwrap();
    let mut o = opts(&dir.path().join("none.sock"));
    o.heartbeat_unconditional = true;
    let g = BreadClient::connect("clip").enable_bug_reporting(o);
    std::thread::sleep(Duration::from_millis(300));
    drop(g); // must not hang or panic
}

#[test]
fn level_names() {
    assert_eq!(level_name("warning"), "WARN");
    assert_eq!(level_name("fatal"), "ERROR");
    assert_eq!(level_name("???"), "INFO");
}
