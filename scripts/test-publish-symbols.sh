#!/usr/bin/env bash
# Behavioral tests for publish-symbols.sh, using real binaries built with rustc.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PUBLISH="$HERE/publish-symbols.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

for tool in rustc readelf objcopy objdump addr2line nm strip stat; do
    command -v "$tool" >/dev/null 2>&1 || fail "$tool is required (install binutils and rustc)"
done

umask 022

cat > "$WORK/a.rs" <<'RS'
#[no_mangle] #[inline(never)] pub extern "C" fn known_fn(x: u32) -> u32 { std::hint::black_box(x) + 1 }
fn main() { std::process::exit(known_fn(0) as i32 & 0); }
RS
cat > "$WORK/b.rs" <<'RS'
#[no_mangle] #[inline(never)] pub extern "C" fn other_fn(x: u32) -> u32 { std::hint::black_box(x) * 3 }
fn main() { std::process::exit(other_fn(0) as i32 & 0); }
RS
rustc -g -C link-arg=-Wl,--build-id -o "$WORK/with-id" "$WORK/a.rs"
rustc -g -C link-arg=-Wl,--build-id=none -o "$WORK/no-id" "$WORK/a.rs"
rustc -g -C link-arg=-Wl,--build-id -o "$WORK/other" "$WORK/b.rs"
strip --strip-all -o "$WORK/stripped" "$WORK/with-id"
objcopy --only-keep-debug "$WORK/other" "$WORK/other.debug"

build_id_of() { readelf -n "$1" 2>/dev/null | sed -n 's/.*Build ID: \([0-9a-f]*\).*/\1/p' | head -n1 || true; }
mode_of() { stat -c %a "$1"; }
has_debug() {
    objdump -h "$1" | awk '
        $2 == ".debug_info" { info = $3 }
        $2 == ".debug_line" { line = $3 }
        END { exit !(info != "" && line != "" && info !~ /^0+$/ && line !~ /^0+$/) }'
}
assert_mode() { [ "$(mode_of "$1")" = "$2" ] || fail "$1 should be mode $2, is $(mode_of "$1")"; }

ID="$(build_id_of "$WORK/with-id")"
[ -n "$ID" ] || fail "test binary has no build id"
OTHER_ID="$(build_id_of "$WORK/other")"
[ "$OTHER_ID" != "$ID" ] || fail "fixture binaries must have different build ids"
dest_in() { echo "$1/${ID:0:2}/${ID}.debug"; }

# 1. default umask 022: root and shard 0750, artifact 0640, build-id preserved
S1="$WORK/store-default"
BREADBUG_SYMBOL_DIR="$S1" bash "$PUBLISH" "$WORK/with-id" >"$WORK/out1"
DEST="$(dest_in "$S1")"
[ -f "$DEST" ] || fail "expected $DEST"
assert_mode "$S1" 750
assert_mode "$S1/${ID:0:2}" 750
assert_mode "$DEST" 640
has_debug "$DEST" || fail "published file has no .debug_info/.debug_line"
[ "$(build_id_of "$DEST")" = "$ID" ] || fail "published file build id differs from the binary"
[ "$(build_id_of "$WORK/stripped")" = "$ID" ] || fail "stripped copy build id differs from the original"
[ "$(build_id_of "$WORK/with-id")" = "$ID" ] || fail "original build id changed"

# 2. addr2line resolves a known function to file:line from the published artifact
ADDR="$(nm "$WORK/with-id" | awk '$3 == "known_fn" { print $1 }')"
[ -n "$ADDR" ] || fail "known_fn not found in test binary"
RES="$(addr2line -f -e "$DEST" "0x$ADDR")"
[ "$(sed -n 1p <<<"$RES")" = known_fn ] || fail "addr2line should resolve known_fn, got: $RES"
sed -n 2p <<<"$RES" | grep -Eq 'a\.rs:1$' || fail "addr2line should resolve to a.rs:1, got: $RES"

# 3. republishing is a no-op and does not rewrite the file
sum1="$(sha256sum "$DEST")"
BREADBUG_SYMBOL_DIR="$S1" bash "$PUBLISH" "$WORK/with-id" >"$WORK/out3"
grep -q "already published" "$WORK/out3" || fail "second publish should say already published"
[ "$sum1" = "$(sha256sum "$DEST")" ] || fail "file changed on republish"

# 4. an existing good artifact with loose permissions is fixed, not skipped
chmod 0644 "$DEST"
BREADBUG_SYMBOL_DIR="$S1" bash "$PUBLISH" "$WORK/with-id" >"$WORK/out4"
grep -q "already published" "$WORK/out4" || fail "loose-mode artifact should still count as published"
assert_mode "$DEST" 640

# 5. permissive caller umask does not change the stored modes
S5="$WORK/store-umask000"
(umask 000; BREADBUG_SYMBOL_DIR="$S5" bash "$PUBLISH" "$WORK/with-id")
assert_mode "$S5" 750
assert_mode "$S5/${ID:0:2}" 750
assert_mode "$(dest_in "$S5")" 640

# 6. pre-existing root or shard that is open to others or group-writable is refused, not changed
S6="$WORK/store-open"
mkdir -m 0755 "$S6"
if BREADBUG_SYMBOL_DIR="$S6" bash "$PUBLISH" "$WORK/with-id" 2>"$WORK/err6"; then fail "world-readable root should be refused"; fi
grep -q "mode 755" "$WORK/err6" || fail "refusal should name the bad mode"
[ ! -e "$(dest_in "$S6")" ] || fail "nothing should be published into an unsafe store"
assert_mode "$S6" 755
chmod 0770 "$S6"
if BREADBUG_SYMBOL_DIR="$S6" bash "$PUBLISH" "$WORK/with-id" 2>/dev/null; then fail "group-writable root should be refused"; fi
chmod 0750 "$S6"
BREADBUG_SYMBOL_DIR="$S6" bash "$PUBLISH" "$WORK/with-id" >/dev/null
[ -f "$(dest_in "$S6")" ] || fail "private root should publish"

# 7. a stripped input fails the job and writes nothing
S7="$WORK/store-stripped"
if BREADBUG_SYMBOL_DIR="$S7" bash "$PUBLISH" "$WORK/stripped" 2>"$WORK/err7"; then fail "stripped input should fail"; fi
grep -q "no usable debug info" "$WORK/err7" || fail "stripped rejection should say why"
[ ! -e "$S7" ] || fail "a rejected stripped input must not create the store"

# 8. mixed run: stripped input fails the job, the good binary is still published
S8="$WORK/store-mixed"
if BREADBUG_SYMBOL_DIR="$S8" bash "$PUBLISH" "$WORK/stripped" "$WORK/with-id" 2>/dev/null; then fail "mixed run with a stripped input should fail"; fi
[ -f "$(dest_in "$S8")" ] || fail "good binary should still be published in a mixed run"
has_debug "$(dest_in "$S8")" || fail "good binary in a mixed run must have debug info"

# 9. unusable existing artifacts for the same build-id are replaced by a valid one
replace_case() {
    local name="$1" source="$2" store="$WORK/store-replace-$1"
    mkdir -p -m 0750 "$store" "$store/${ID:0:2}"
    cp "$source" "$(dest_in "$store")"
    BREADBUG_SYMBOL_DIR="$store" bash "$PUBLISH" "$WORK/with-id" >"$WORK/out-$name" 2>"$WORK/err-$name" \
        || fail "$name: publish should replace the unusable artifact"
    grep -q "replacing unusable artifact" "$WORK/err-$name" || fail "$name: should say it replaced the artifact"
    has_debug "$(dest_in "$store")" || fail "$name: replacement has no debug info"
    [ "$(build_id_of "$(dest_in "$store")")" = "$ID" ] || fail "$name: replacement has the wrong build id"
    assert_mode "$(dest_in "$store")" 640
    [ -z "$(find "$store" -name '.*' -print -quit)" ] || fail "$name: temp files left behind"
}
replace_case stripped "$WORK/stripped"
replace_case empty /dev/null
replace_case wrong-id "$WORK/other.debug"

# 10. a bad existing entry is not treated as an idempotent success
S10="$WORK/store-bad-entry-fails"
mkdir -p -m 0750 "$S10" "$S10/${ID:0:2}"
cp "$WORK/stripped" "$(dest_in "$S10")"
if BREADBUG_SYMBOL_DIR="$S10" bash "$PUBLISH" "$WORK/stripped" 2>/dev/null; then fail "stripped input must not be accepted over a bad entry"; fi
has_debug "$(dest_in "$S10")" && fail "bad entry should not be silently turned into good data"

# 11. a binary without a build-id fails the job by default, publishes nothing
S11="$WORK/store-noid"
if BREADBUG_SYMBOL_DIR="$S11" bash "$PUBLISH" "$WORK/no-id" 2>"$WORK/err11"; then fail "missing build-id should fail"; fi
grep -q "no GNU build-id" "$WORK/err11" || fail "error should say why"
[ -z "$(find "$S11" -name '*.debug' 2>/dev/null)" ] || fail "nothing should be published"

# 12. REQUIRE_BUILD_ID=0 only warns
REQUIRE_BUILD_ID=0 BREADBUG_SYMBOL_DIR="$WORK/store-warn" bash "$PUBLISH" "$WORK/no-id" 2>"$WORK/err12" \
    || fail "should not fail with REQUIRE_BUILD_ID=0"
grep -q "skipping" "$WORK/err12" || fail "should warn"

# 13. missing file, no arguments, relative store path
if BREADBUG_SYMBOL_DIR="$WORK/store-missing" bash "$PUBLISH" "$WORK/nope" 2>/dev/null; then fail "missing file should fail"; fi
if BREADBUG_SYMBOL_DIR="$WORK/store-args" bash "$PUBLISH" 2>/dev/null; then fail "no args should fail"; fi
if BREADBUG_SYMBOL_DIR="relative/store" bash "$PUBLISH" "$WORK/with-id" 2>/dev/null; then fail "relative store path should fail"; fi

# 14. the server-side lookup path matches: <xx>/<id>.debug with lowercase hex
echo "$ID" | grep -qE '^[0-9a-f]{8,128}$' || fail "build id not lowercase hex"

echo "publish-symbols: all tests passed"
