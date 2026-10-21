#!/usr/bin/env bash
# Copies what later workflow steps use out of the persistent cargo target
# cache into the job's checkout. Run by ci/build.sh inside the container,
# after the product's command.
#
# Usage: copy-artifacts.sh <cache target dir> <dest target dir>
#
# build.sh used to `cp -a` the whole cache back after every command. The
# cache persists between runs and holds every profile's deps/, incremental/
# and .fingerprint/: for a Tauri app (breadhelp) that is about 10 GB and
# 16k files, copied twice per run (build step, test step), which turned the
# end of each step into a multi-minute stall. Workflows only ever read
#
#   - target/<profile>/<binary>: the artifacts (every product), and
#   - target/release/build/<crate>-<hash>/out/: build-script output
#     (breadsearch takes ONNX Runtime's LICENSE and ThirdPartyNotices.txt
#     from breadmill's),
#
# so that is what gets copied. Steps that need more read /cargo-target
# inside the container (breadsearch's check does), or set
# BREAD_CI_COPY_TARGET=all to restore the old full copy.
set -euo pipefail

SRC="$1"
DEST="$2"
mkdir -p "$DEST"

if [ "${BREAD_CI_COPY_TARGET:-artifacts}" = "all" ]; then
    cp -a "$SRC/." "$DEST/"
    exit 0
fi

# Profile directories (debug/, release/, custom profiles) are the ones
# cargo gave a deps/ directory. Their top-level regular files are the
# artifacts; subdirectories are cargo's own bookkeeping.
for profile in "$SRC"/*/; do
    profile="${profile%/}"
    [ -d "$profile/deps" ] || continue
    name="${profile##*/}"
    mkdir -p "$DEST/$name"
    find "$profile" -maxdepth 1 -type f -exec cp -a -t "$DEST/$name/" {} +
done

for out in "$SRC"/release/build/*/out; do
    [ -d "$out" ] || continue
    rel="${out#"$SRC"/}"
    mkdir -p "$DEST/$rel"
    cp -a "$out/." "$DEST/$rel/"
done
