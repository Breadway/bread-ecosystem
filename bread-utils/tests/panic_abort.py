"""Run with python3 bread-utils/tests/panic_abort.py.

Cargo's test harness uses unwinding even for panic=abort release profiles.
Build a real consumer instead, then check setup and a genuine panic against
a private socket. No report reaches the user's daemon.
"""

import json
import os
from pathlib import Path
import resource
import signal
import socket
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]
resource.setrlimit(resource.RLIMIT_CORE, (0, 0))

with tempfile.TemporaryDirectory(prefix="bug-abort-") as scratch:
    work = Path(scratch)
    (work / "src").mkdir()
    (work / "Cargo.toml").write_text(
        '[package]\nname = "bug-abort-fixture"\nversion = "0.0.0"\n'
        'edition = "2021"\n[dependencies]\n'
        f'bread-utils = {{ path = {json.dumps(str(ROOT / "bread-utils"))}, '
        'features = ["bread-client"] }\n[profile.dev]\npanic = "abort"\n'
    )
    (work / "src/main.rs").write_text(
        'fn main() {\n'
        '    assert!(cfg!(panic = "abort"));\n'
        '    let _client = bread_utils::bread_client::BreadClient::connect("crumbs");\n'
        '    println!("initialized");\n'
        '    if std::env::args().any(|a| a == "panic") {\n'
        '        panic!("abort fixture panic");\n'
        '    }\n'
        '}\n'
    )
    target = ROOT / "target/panic-abort-fixture"
    subprocess.run(
        ["cargo", "build", "--offline", "--manifest-path", str(work / "Cargo.toml"),
         "--target-dir", str(target)], check=True,
    )
    binary = target / "debug/bug-abort-fixture"
    runtime = work / "runtime"
    (runtime / "bread").mkdir(parents=True)
    env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), BREAD_BUG_REPORTING="1")
    with socket.socket(socket.AF_UNIX) as listener:
        listener.bind(str(runtime / "bread/breadd.sock"))
        listener.listen(20)
        listener.settimeout(1)
        setup = subprocess.run([str(binary)], env=env, capture_output=True, timeout=5)
        assert setup.returncode == 0, setup
        assert b"initialized" in setup.stdout, setup
        crashed = subprocess.run(
            [str(binary), "panic"], env=env, capture_output=True, timeout=5,
        )
        assert crashed.returncode == -signal.SIGABRT, crashed
        assert b"initialized" in crashed.stdout, crashed
        assert b"abort fixture panic" in crashed.stderr, crashed
        requests = []
        while True:
            try:
                conn, _ = listener.accept()
            except TimeoutError:
                break
            with conn:
                conn.settimeout(1)
                with conn.makefile() as lines:
                    requests.extend(json.loads(line) for line in lines)
        panics = [r["params"]["marker"] for r in requests
                  if r["method"] == "bug.marker"
                  and r["params"]["marker"]["kind"] == "panic"]
        assert len(panics) == 1, requests
        assert panics[0]["frames"], panics
        assert "main.rs" in panics[0]["location"], panics
    print("panic=abort: setup survives; real panic reports location and stack")
