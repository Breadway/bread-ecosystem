#!/usr/bin/env bash
# Tests for publish-symbols.sh, using real binaries built with rustc.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

cat > "$WORK/a.rs" <<'RS'
#[no_mangle] #[inline(never)] pub extern "C" fn known_fn(x: u32) -> u32 { std::hint::black_box(x) + 1 }
fn main() { std::process::exit(known_fn(0) as i32 & 0); }
RS
rustc -g -C link-arg=-Wl,--build-id -o "$WORK/with-id" "$WORK/a.rs"
rustc -g -C link-arg=-Wl,--build-id=none -o "$WORK/no-id" "$WORK/a.rs"
STORE="$WORK/store"

# 1. publishes under <xx>/<build-id>.debug and keeps the debug info
export BREADBUG_SYMBOL_DIR="$STORE"
bash "$HERE/publish-symbols.sh" "$WORK/with-id" >"$WORK/out1"
ID="$(readelf -n "$WORK/with-id" | sed -n 's/.*Build ID: \([0-9a-f]*\).*/\1/p' | head -n1)"
[ -n "$ID" ] || fail "test binary has no build id"
DEST="$STORE/${ID:0:2}/${ID}.debug"
[ -f "$DEST" ] || fail "expected $DEST"
readelf -S "$DEST" 2>/dev/null | grep -q '\.debug_info' || fail "published file has no DWARF"
[ "$(stat -c %a "$DEST")" = 644 ] || fail "mode should be 0644"

# 2. republishing is a no-op and does not rewrite the file
sum1="$(sha256sum "$DEST")"
bash "$HERE/publish-symbols.sh" "$WORK/with-id" >"$WORK/out2"
grep -q "already published" "$WORK/out2" || fail "second publish should say already published"
[ "$sum1" = "$(sha256sum "$DEST")" ] || fail "file changed on republish"

# 3. a binary without a build-id fails the job by default, publishes nothing
rm -rf "$STORE"
if bash "$HERE/publish-symbols.sh" "$WORK/no-id" 2>"$WORK/err3"; then fail "missing build-id should fail"; fi
grep -q "no GNU build-id" "$WORK/err3" || fail "error should say why"
[ -z "$(find "$STORE" -name '*.debug' 2>/dev/null)" ] || fail "nothing should be published"

# 4. REQUIRE_BUILD_ID=0 only warns
REQUIRE_BUILD_ID=0 bash "$HERE/publish-symbols.sh" "$WORK/no-id" 2>"$WORK/err4" || fail "should not fail with REQUIRE_BUILD_ID=0"
grep -q "skipping" "$WORK/err4" || fail "should warn"

# 5. one bad binary does not stop the others, but the job still fails
rm -rf "$STORE"
if bash "$HERE/publish-symbols.sh" "$WORK/no-id" "$WORK/with-id" 2>/dev/null; then fail "mixed run should fail"; fi
[ -f "$DEST" ] || fail "good binary should still be published"

# 6. missing file and no arguments
if bash "$HERE/publish-symbols.sh" "$WORK/nope" 2>/dev/null; then fail "missing file should fail"; fi
if bash "$HERE/publish-symbols.sh" 2>/dev/null; then fail "no args should fail"; fi

# 7. the server-side lookup path matches: <xx>/<id>.debug with lowercase hex
echo "$ID" | grep -qE '^[0-9a-f]{8,128}$' || fail "build id not lowercase hex"

echo "publish-symbols: all tests passed"
