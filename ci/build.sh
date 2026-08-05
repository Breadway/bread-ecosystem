#!/usr/bin/env bash
# Shared CI build script for bread-ecosystem GTK4/libadwaita apps.
#
# Builds (or reuses, via docker's own layer cache) the pinned Arch image
# from ci/Containerfile, then runs the given cargo command inside it
# against a product repo checkout.
#
# Usage: ci/build.sh <product-name> <product-repo-root> <cargo-command...>
#   e.g. ci/build.sh breadpad /path/to/breadpad cargo build --release --locked
#
# <product-name> is used verbatim as the image tag and cache-volume name —
# it must be passed explicitly rather than derived from <product-repo-root>'s
# basename, because every product's CI checks out into a directory literally
# named `src`, which would otherwise collide across every product sharing
# this runner (same image tag, same cargo-target cache volume).
#
# If <product-repo-root>/ci/deps.txt exists (one pacman package per line,
# '#' comments and blank lines ignored), those packages are installed on
# top of the shared base image.
#
# Cargo's registry/git caches are shared across all products (same crates
# regardless of which app is building); CARGO_TARGET_DIR is cached
# per-product. Both persist in named docker volumes across runs.
set -euo pipefail

if [ $# -lt 3 ]; then
    echo "usage: build.sh <product-name> <product-repo-root> <cargo-command...>" >&2
    exit 1
fi

PRODUCT="$1"
REPO_ROOT="$(cd "$2" && pwd)"
shift 2

CI_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

EXTRA_PKGS=""
if [ -f "${REPO_ROOT}/ci/deps.txt" ]; then
    EXTRA_PKGS="$(grep -vE '^\s*(#|$)' "${REPO_ROOT}/ci/deps.txt" | tr '\n' ' ')"
fi

docker build \
    --build-arg "EXTRA_PKGS=${EXTRA_PKGS}" \
    -t "bread-ci:${PRODUCT}" \
    -f "${CI_DIR}/Containerfile" "${CI_DIR}"

docker run --rm \
    -v "${REPO_ROOT}:/workspace" \
    -v "bread-ci-cargo-registry:/root/.cargo/registry" \
    -v "bread-ci-cargo-git:/root/.cargo/git" \
    -v "bread-ci-${PRODUCT}-target:/cargo-target" \
    -w /workspace \
    -e CARGO_TARGET_DIR=/cargo-target \
    "bread-ci:${PRODUCT}" \
    bash -c '
        set -euo pipefail
        "$@"
        mkdir -p /workspace/target
        cp -a /cargo-target/. /workspace/target/
    ' bash "$@"
