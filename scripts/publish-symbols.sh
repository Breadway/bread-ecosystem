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
#   BREADBUG_SYMBOL_DIR   absolute store root (default /srv/breadbug-symbols);
#                         files go to <root>/<first two hex>/<build-id>.debug
#   REQUIRE_BUILD_ID      1 (default): a binary without a build-id fails the
#                         job. 0: warn and skip it.
#
# The store is append-only on purpose: every released version stays
# symbolicatable for as long as it is installable. Re-publishing a build-id
# whose stored file is usable is a no-op. A stored file for the same build-id
# that is unusable (no line tables, wrong build-id, not a regular file) is
# replaced atomically with one extracted from the binary.
#
# Only usable symbols are published: the input and the extracted file must
# both carry non-empty .debug_info and .debug_line sections. A stripped input
# fails the job and writes nothing.
#
# Permissions: directories are 0750 and files are 0640, set explicitly, so the
# caller's umask does not matter. The store is readable by the writer and by a
# reader group (see docs/breadbug-symbols.md). An existing root or shard
# directory that is a symlink, grants others any access, or is group-writable
# is refused rather than changed.
#
# Requires: readelf, objcopy, objdump (binutils)
set -euo pipefail

SYMBOL_DIR="${BREADBUG_SYMBOL_DIR:-/srv/breadbug-symbols}"
REQUIRE="${REQUIRE_BUILD_ID:-1}"
DIR_MODE=0750
FILE_MODE=0640
umask 027

if [ $# -eq 0 ]; then
    echo "usage: publish-symbols.sh <binary>..." >&2
    exit 2
fi

case "$SYMBOL_DIR" in
    /*) ;;
    *)
        echo "::error::publish-symbols: BREADBUG_SYMBOL_DIR must be an absolute path" >&2
        exit 2
        ;;
esac

for tool in readelf objcopy objdump; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "::error::publish-symbols: $tool not found (install binutils)" >&2
        exit 2
    fi
done

build_id() {
    readelf -n "$1" 2>/dev/null | sed -n 's/^[[:space:]]*Build ID:[[:space:]]*\([0-9a-f]\{8,128\}\)[[:space:]]*$/\1/p' | head -n1 || true
}

debug_size() {
    objdump -h "$1" 2>/dev/null | awk -v s="$2" '$2 == s { print $3; exit }' || true
}

has_line_tables() {
    local info line
    info="$(debug_size "$1" .debug_info)"
    line="$(debug_size "$1" .debug_line)"
    [ -n "$info" ] && [ -n "$line" ] && [ $((16#$info)) -gt 0 ] && [ $((16#$line)) -gt 0 ]
}

store_ready() {
    local shard="$1" d mode
    if [ -L "$SYMBOL_DIR" ] || [ -L "$shard" ]; then
        echo "::error::publish-symbols: $SYMBOL_DIR and $shard must be real directories, not symlinks" >&2
        return 1
    fi
    mkdir -p -m "$DIR_MODE" "$SYMBOL_DIR" "$shard"
    for d in "$SYMBOL_DIR" "$shard"; do
        mode="$(stat -c %a "$d")"
        if [ ! -d "$d" ] || [ $((8#$mode & 027)) -ne 0 ]; then
            echo "::error::publish-symbols: $d has mode $mode; it must not grant access to others or write to group (fix with: chmod 0750 $d)" >&2
            return 1
        fi
    done
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
    if ! has_line_tables "$bin"; then
        echo "::error::publish-symbols: $bin has no usable debug info (.debug_info or .debug_line is missing or empty); build with debug = \"line-tables-only\" and publish before strip" >&2
        status=1
        continue
    fi
    dest_dir="${SYMBOL_DIR}/${id:0:2}"
    dest="${dest_dir}/${id}.debug"
    if ! store_ready "$dest_dir"; then
        status=1
        continue
    fi
    if [ -f "$dest" ] && [ ! -L "$dest" ] && has_line_tables "$dest" && [ "$(build_id "$dest")" = "$id" ]; then
        if ! chmod "$FILE_MODE" "$dest"; then
            echo "::error::publish-symbols: cannot set mode $FILE_MODE on $dest" >&2
            status=1
            continue
        fi
        echo "publish-symbols: $(basename "$bin") ${id} already published"
        continue
    fi
    if [ -e "$dest" ] || [ -L "$dest" ]; then
        echo "publish-symbols: warning: replacing unusable artifact $dest" >&2
    fi
    tmp="$(mktemp "${dest_dir}/.${id}.XXXXXX")"
    if ! objcopy --only-keep-debug "$bin" "$tmp" || ! has_line_tables "$tmp" || [ "$(build_id "$tmp")" != "$id" ]; then
        rm -f "$tmp"
        echo "::error::publish-symbols: could not extract usable debug info from $bin (objcopy failed, or the output lacks .debug_info/.debug_line or build-id $id)" >&2
        status=1
        continue
    fi
    if ! chmod "$FILE_MODE" "$tmp" || ! chgrp "$(stat -c %g "$dest_dir")" "$tmp" || ! mv -fT "$tmp" "$dest"; then
        rm -f "$tmp"
        echo "::error::publish-symbols: could not install $dest" >&2
        status=1
        continue
    fi
    echo "publish-symbols: $(basename "$bin") ${id} -> ${dest}"
done
exit "$status"
