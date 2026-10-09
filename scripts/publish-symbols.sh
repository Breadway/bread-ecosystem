#!/usr/bin/env bash
# Publish debug symbols for release binaries to the breadbug symbol store,
# keyed by GNU build-id, so breadbug-server can turn the (module, offset,
# build-id) frames in crash reports back into function, file and line while
# the shipped binaries stay stripped.
#
# Usage: scripts/publish-symbols.sh <binary>...
#   Call it on the UNSTRIPPED release binaries, before `strip`.
#
# Environment:
#   BREADBUG_SYMBOL_DIR   store root (default /srv/breadbug-symbols); files go
#                         to <root>/<first two hex>/<build-id>.debug
#   REQUIRE_BUILD_ID      1 (default): a binary without a build-id fails the
#                         job. 0: warn and skip it.
#
# The store is append-only on purpose: every released version stays
# symbolicatable for as long as it is installable. Re-publishing the same
# build-id is a no-op (the id identifies the exact build).
#
# Requires: readelf, objcopy
set -euo pipefail

SYMBOL_DIR="${BREADBUG_SYMBOL_DIR:-/srv/breadbug-symbols}"
REQUIRE="${REQUIRE_BUILD_ID:-1}"

if [ $# -eq 0 ]; then
    echo "usage: publish-symbols.sh <binary>..." >&2
    exit 2
fi

build_id() {
    readelf -n "$1" 2>/dev/null | sed -n 's/^[[:space:]]*Build ID:[[:space:]]*\([0-9a-f]\{8,128\}\)[[:space:]]*$/\1/p' | head -n1
}

status=0
for bin in "$@"; do
    if [ ! -f "$bin" ]; then
        echo "::error::publish-symbols: $bin does not exist" >&2
        status=1
        continue
    fi
    id="$(build_id "$bin")"
    if [ -z "$id" ]; then
        if [ "$REQUIRE" = "1" ]; then
            echo "::error::publish-symbols: $bin has no GNU build-id; crash reports from it could not be symbolicated (link with -C link-arg=-Wl,--build-id)" >&2
            status=1
        else
            echo "publish-symbols: warning: $bin has no build-id, skipping" >&2
        fi
        continue
    fi
    dest_dir="${SYMBOL_DIR}/${id:0:2}"
    dest="${dest_dir}/${id}.debug"
    if [ -f "$dest" ]; then
        echo "publish-symbols: $(basename "$bin") ${id} already published"
        continue
    fi
    mkdir -p "$dest_dir"
    tmp="$(mktemp "${dest_dir}/.${id}.XXXXXX")"
    if ! objcopy --only-keep-debug "$bin" "$tmp"; then
        rm -f "$tmp"
        echo "::error::publish-symbols: objcopy failed for $bin" >&2
        status=1
        continue
    fi
    chmod 0644 "$tmp"
    mv -n "$tmp" "$dest"
    rm -f "$tmp"
    echo "publish-symbols: $(basename "$bin") ${id} -> ${dest}"
done
exit "$status"
