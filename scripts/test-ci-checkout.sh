#!/usr/bin/env bash
# Behavioral tests for ci-checkout.sh, using a local file:// origin.
#
# Covers the ECO-79 race: two jobs on different branches must each keep their
# own tree while the other clone runs, the branch can advance after the event
# and the event SHA is still honored, and a failed checkout leaves no directory
# behind.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECKOUT="$HERE/ci-checkout.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid

ORIGIN="$WORK/origin"
git init -q -b main "$ORIGIN"
mkdir -p "$ORIGIN/crate/src"
printf 'pub mod livestack;\npub mod procid;\n' > "$ORIGIN/crate/src/lib.rs"
echo 'pub fn live() {}' > "$ORIGIN/crate/src/livestack.rs"
echo 'pub fn proc() {}' > "$ORIGIN/crate/src/procid.rs"
git -C "$ORIGIN" add -A && git -C "$ORIGIN" commit -qm base
git -C "$ORIGIN" checkout -q -b feature/with-modules
git -C "$ORIGIN" checkout -q main
git -C "$ORIGIN" checkout -q -b feature/without-modules
git -C "$ORIGIN" rm -q crate/src/livestack.rs crate/src/procid.rs
printf '' > "$ORIGIN/crate/src/lib.rs"
git -C "$ORIGIN" add -A && git -C "$ORIGIN" commit -qm "drop modules"
WITH_SHA="$(git -C "$ORIGIN" rev-parse feature/with-modules)"
WITHOUT_SHA="$(git -C "$ORIGIN" rev-parse feature/without-modules)"
[ "$WITH_SHA" != "$WITHOUT_SHA" ] || fail "fixture branches must differ"

# Exact-SHA checkout: each job gets its event commit's tree, not the branch tip.
URL="file://$ORIGIN"
"$CHECKOUT" "$URL" "$WITH_SHA" "$WORK/job-with"
[ -f "$WORK/job-with/crate/src/livestack.rs" ] || fail "with-modules checkout is missing livestack.rs"
[ "$(git -C "$WORK/job-with" rev-parse HEAD)" = "$WITH_SHA" ] || fail "HEAD does not match the event sha"
rm -rf "$WORK/job-with"

# Branch advances after the event: the event sha must still be honored.
git -C "$ORIGIN" checkout -q feature/with-modules
echo 'pub fn later() {}' > "$ORIGIN/crate/src/later.rs"
git -C "$ORIGIN" add -A && git -C "$ORIGIN" commit -qm "advance after event"
ADVANCED_SHA="$(git -C "$ORIGIN" rev-parse feature/with-modules)"
[ "$ADVANCED_SHA" != "$WITH_SHA" ] || fail "branch did not advance"
"$CHECKOUT" "$URL" "$WITH_SHA" "$WORK/job-advanced"
[ ! -e "$WORK/job-advanced/crate/src/later.rs" ] || fail "checkout followed the advanced branch, not the event sha"
[ "$(git -C "$WORK/job-advanced" rev-parse HEAD)" = "$WITH_SHA" ] || fail "advanced-branch HEAD mismatch"
rm -rf "$WORK/job-advanced"

# Concurrency: two jobs on different branches, each in its own mktemp dir. Job A
# checks its tree after job B's clone has finished, which is the window where
# the old shared-path checkout lost files.
A_DIR="$(mktemp -d "$WORK/job-a.XXXXXX")"
rmdir "$A_DIR"
B_DIR="$(mktemp -d "$WORK/job-b.XXXXXX")"
rmdir "$B_DIR"
(
    "$CHECKOUT" "$URL" "$WITH_SHA" "$A_DIR"
    sleep 1
    [ -f "$A_DIR/crate/src/livestack.rs" ] || { echo "job A tree was replaced" >&2; exit 1; }
) &
pid_a=$!
(
    sleep 0.2
    "$CHECKOUT" "$URL" "$WITHOUT_SHA" "$B_DIR"
) &
pid_b=$!
wait "$pid_a" || fail "job A lost its tree during the concurrent clone"
wait "$pid_b" || fail "job B checkout failed"
[ ! -e "$B_DIR/crate/src/livestack.rs" ] || fail "job B tree is wrong"
rm -rf "$A_DIR" "$B_DIR"

# Control: the pre-ECO-79 pattern (shared relative path, branch clone) must
# lose job A's file under the same schedule. If it does not, this scenario no
# longer exercises the race and the test above proves nothing.
OLD="$WORK/old-shared"
mkdir "$OLD"
(
    cd "$OLD"
    rm -rf src && git clone -q --branch feature/with-modules --depth 1 "$URL" src
    sleep 1
    if [ -f src/crate/src/livestack.rs ]; then exit 0; else exit 1; fi
) &
old_a=$!
(
    sleep 0.2
    cd "$OLD"
    rm -rf src && git clone -q --branch feature/without-modules --depth 1 "$URL" src
) &
old_b=$!
old_a_rc=0; wait "$old_a" || old_a_rc=$?
wait "$old_b" || true
[ "$old_a_rc" -ne 0 ] || fail "control did not reproduce the shared-path race; the regression is not meaningful"

# Failure cleanup: a bad sha must not leave the destination behind.
BAD_DEST="$WORK/job-bad"
if "$CHECKOUT" "$URL" "0000000000000000000000000000000000000001" "$BAD_DEST" 2>/dev/null; then
    fail "checkout of an unknown sha should fail"
fi
[ ! -e "$BAD_DEST" ] || fail "failed checkout left its directory behind"

# Input validation: short sha, existing destination.
expect_rc() {
    local want="$1" label="$2"; shift 2
    local rc=0
    "$@" >/dev/null 2>&1 || rc=$?
    [ "$rc" -eq "$want" ] || fail "$label: expected exit $want, got $rc"
}
expect_rc 2 "short sha" "$CHECKOUT" "$URL" "abc1234" "$WORK/job-short"
[ ! -e "$WORK/job-short" ] || fail "short sha created a directory"
mkdir "$WORK/job-exists"
expect_rc 2 "existing destination" "$CHECKOUT" "$URL" "$WITH_SHA" "$WORK/job-exists"
rmdir "$WORK/job-exists"

# Userinfo is rejected before any network access. Token-only, user-only, and
# user:password forms are all refused. The secret strings are fake fixtures.
# This runs with an unreachable host, so a rejection that happened after the
# network would fail for the wrong reason and exit with another code.
for bad in \
    "https://FAKE-TOKEN-ONLY@127.0.0.1:1/x.git" \
    "https://FAKEUSER@127.0.0.1:1/x.git" \
    "https://FAKEUSER:FAKEPASS@127.0.0.1:1/x.git" \
    "ssh://git@127.0.0.1:1/x.git"; do
    expect_rc 2 "userinfo ${bad%%@*}@... rejected" "$CHECKOUT" "$bad" "$WITH_SHA" "$WORK/job-cred"
    [ ! -e "$WORK/job-cred" ] || fail "userinfo rejection created a directory"
done

# scp-style SSH (host:path) carries no userinfo and must pass the URL check.
# It exits with a non-validation code because the host is unreachable; the
# assertion is that it is not 2, the input-rejection code.
rc=0
"$CHECKOUT" "git@127.0.0.1:1/none.git" "$WITH_SHA" "$WORK/job-ssh" >/dev/null 2>&1 || rc=$?
[ "$rc" -ne 2 ] || fail "scp-style ssh URL was rejected as invalid input"
[ ! -e "$WORK/job-ssh" ] || fail "failed ssh attempt left its directory behind"

echo "ci-checkout.sh: exact-sha, concurrent, advance, cleanup, and validation tests passed"
