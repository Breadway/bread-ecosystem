# breadbug client (bread-utils `bug` module)

How a bread app reports crashes, freezes and logs to breadd, what leaves the
process, and what it costs. The design is ECO-66; this page covers the client
side (BREAD-16 and BREAD-17).

## Adoption

Reporting is on by default. `BreadClient::connect` starts it for the process:

```rust
let client = BreadClient::connect("clip"); // panic hook, crash handlers, logs, heartbeat
bread_utils::bug::tick();                  // non-GTK apps: once per main-loop pass
```

GTK apps built with the `gtk` feature tick the heartbeat from the GLib main
context automatically, so they need no `tick()` call. That GLib source is
removed when the guard is dropped.

Opt-outs:

- Code: `BreadClient::connect_without_bug_reporting("pass")` for apps that
  handle sensitive data. They can still call `enable_bug_reporting` later.
- User or environment: `BREAD_BUG_REPORTING=0` (also `off`, `false`, `no`,
  `disabled`) turns the default off for the process.
- Custom options: `client.enable_bug_reporting(BugOptions { .. })`. It replaces
  the default and returns a guard that owns the threads.

The main-loop call is the only per-app step beyond `connect`. Build release
binaries with a GNU build-id (`RUSTFLAGS="-C link-arg=-Wl,--build-id"`), and
with frame pointers (`-C force-frame-pointers=yes`) for complete native stacks.
`bug::has_build_id()` reports whether the running binary has one.

## Privacy: what leaves the process

Redaction runs in the client before anything reaches the socket
(`bug::scrub`). It replaces:

- the home directory with `~`, and the user name with `<user>`;
- `/home/<name>`, `/Users/<name>` and `/run/user/<uid>` roots;
- e-mail addresses (`<email>`), IPv4 addresses (`<ip>`);
- token-looking words with well-known prefixes (`<secret>`), and `key=value`
  pairs whose key looks secret (`token=<redacted>`);
- hex identifiers of 32 or more characters (`<id>`).

It applies to panic messages, panic locations, module paths in stacks, native
crash module paths, and log lines. The client never reads or sends environment
variables; `HOME` and the user name are read only to know what to hide.

This is a denylist, so it is the first of three layers (ECO-66, "scrub in two
places"):

1. **Client (this module):** redacts before sending, as above.
2. **breadd ingest:** scrubs with an allowlist before storing.
3. **Report time:** the assembled report is scrubbed again before it can be
   sent anywhere.

Layers 2 and 3 are not implemented in this branch. Until they land, the client
layer is the only one, so it is not a complete guarantee on its own.

## Crash handling

**Panics** use a panic hook that captures frames with `backtrace(3)`, outside
any signal context, and sends them on a helper thread with a 400 ms deadline.

**Native crashes** (SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGABRT) use an
in-process signal handler, chosen over systemd-coredump because it needs no
privileges or system setup. The handler:

- is async-signal-safe: it calls only `sigaction`, `raise`, `socket`,
  `connect`, `send`, `close`, and `read`/`write` on a pre-made pipe. It takes
  no locks, allocates nothing, and formats into a fixed 16 KiB stack buffer.
- does not use `backtrace(3)`. Its unwinder may `dlopen` libgcc and takes the
  loader lock, so a fault inside `malloc` or the loader would deadlock. It
  walks the frame-pointer chain from the faulting `ucontext` (RIP and RBP on
  x86_64 Linux) instead. Each word is copied through a pipe, which returns
  `EFAULT` on a bad address, so a corrupt stack ends the walk without a second
  fault. Frame pointers are required for full stacks.
- saves the prior `sigaction` for each signal and checks every syscall. If
  setup fails, the actions already replaced are restored, a warning is logged,
  and `install` returns `false`.
- chains to the prior action after reporting:
  - `SIG_DFL` (or none installed): restored, then re-executed for a fault
    (`si_code > 0`) or raised for a signal the process sent to itself. The
    process dies of the original signal.
  - `SIG_IGN`: a fault terminates the process, since returning would retry
    forever.
  - Any other handler, such as a C library's: called with the original
    arguments, so it sees the real fault.
  - **Rust std's stack-overflow guard handler is the exception.** It is
    recognized at install time: a real handler (not `SIG_DFL`/`SIG_IGN`) with
    `SA_SIGINFO | SA_ONSTACK` whose address is in this binary's own executable
    (loader object matched by device and inode, not path text). std installs it
    during runtime init, before any app code runs, so by install time it is the
    only handler that matches. It is treated as `SIG_DFL`, because chaining to
    it aborts, which turns a SIGSEGV into SIGABRT. The trade-off: a stack
    overflow is reported as a plain crash rather than as "overflowed its stack"
    (the report is still written from the alternate stack), and an app handler
    in its own binary with those same flags, installed before this module,
    would be mistaken for std's. C-library handlers live in another object and
    never match; custom handlers without `SA_ONSTACK` never match. Every other
    prior handler keeps chaining.
- uses an alternate stack. An existing one is kept only if it is at least
  128 KiB; otherwise a 128 KiB stack is installed. Other threads need their own
  stack to report a stack overflow.

Frames only: module, offset and build-id. No memory contents, no registers
beyond the instruction and frame pointers, no environment, and no core files
are ever read.

## Logs and shutdown

- Log lines go to a bounded queue of 512. A full queue drops the **oldest**
  line, so the most recent context is kept. Producers never wait.
- The sender is rate limited to 50 lines/s sustained, with a 200-line burst,
  in batches of at most 50.
- Each record is formatted into a buffer capped at 2 KiB, so a huge `Display`
  impl cannot allocate without bound.
- Dropping the guard sets a stop flag and waits at most 500 ms for the threads
  to finish, whether or not a `BugLogger` clone is still alive. A clone
  holding the channel open cannot delay shutdown.
- Connects use a non-blocking socket, and writes time out after 200 ms.

## Overhead

Measured with:

```text
cargo run --release -p bread-utils --features bread-client --locked --example bug_overhead
```

Machine: AMD Ryzen AI 7 350 (16 logical CPUs), Linux, rustc 1.97.1, release
profile. Seven runs; the table shows median, min and max in nanoseconds per
call. Each run is one million `tick()` calls and 100,000 `log()` calls against
a socket nothing listens on (breadd absent), so the queue is full and lines
are dropped, which is the steady state for that case.

| Call | median | min | max |
|---|---|---|---|
| `BugGuard::tick()` | 48.9 ns | 48.6 ns | 55.1 ns |
| `BugGuard::log()` | 869.6 ns | 827.1 ns | 1027.4 ns |
| `log::Log::log()` (via `BugLogger`) | 1204.9 ns | 1081.1 ns | 1655.4 ns |

What these numbers mean:

- `tick()` is an atomic store plus a clock read. Per frame, at 60 Hz, it is
  well under 0.01% of a frame.
- `log()` is dominated by redaction, which runs on the caller's thread before
  the line is queued. The `log::Log` path adds the `format!` into a bounded
  buffer. Both are paid per line, so high-volume logging costs about a
  microsecond per line on this machine.
- Background sending runs on its own thread and is not in these numbers.
- The spread between runs is large (the caller races the worker for the queue
  lock). The median is the figure to compare against.

Not measured: a live breadd (send cost), the GLib tick under a busy UI, and
other CPUs.

## Verification status

Verified in this repository by `cargo test`:

- panic capture and redaction, including the fake e-mail, secret and home path
  cases;
- crash capture with real SIGSEGV and a real panic in child processes;
- default-action termination (dies of SIGSEGV);
- chaining to a custom, non-std prior SIGSEGV handler, which still runs;
- setup failure leaves signal actions unchanged;
- queue overflow drops the oldest line; the sender stays within the rate
  budget; shutdown is bounded while a logger clone is alive.

Not verified here, and still open:

- the breadd store entry and the induced breadclip pilot panic (bread PR22 and
  CLIP-19 are not merged);
- breadd-side allowlist scrubbing and report-time scrubbing (layers 2 and 3);
- stack-overflow reports (the guard-page case, documented above).
