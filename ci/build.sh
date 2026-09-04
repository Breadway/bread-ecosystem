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
# per-product. Both persist as host directories under CACHE_ROOT (bind
# mounts, not named docker volumes — see the --user note below) across runs.
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

# The container runs as the invoking user (--user), not root, so that
# `cp -a` below (and anything cargo writes) doesn't leave host-owned-by-root
# files behind in ${REPO_ROOT}/target. That only works if every writable
# mount the container touches is already writable by that user:
#
#   - ${REPO_ROOT} is a bind mount of a real host directory the invoking
#     user already owns, so it's fine as-is.
#   - The cargo registry/git/target caches used to be named docker volumes
#     mounted under /root/.cargo/... and /cargo-target. Two problems with
#     that combination: (1) /root is 0750 root:root in the base image, so a
#     non-root, non-root-group user can't even traverse into
#     /root/.cargo/registry regardless of that directory's own permissions;
#     (2) dockerd's local volume driver initializes a fresh named volume as
#     root:root 0755 and does not chown it to match --user, so a top-level
#     mount like /cargo-target has the same problem. Bind-mounting host
#     directories under CACHE_ROOT instead sidesteps both: `mkdir -p` below
#     runs as the invoking user, so the directories are already theirs
#     before the container ever starts.
CACHE_ROOT="${BREAD_CI_CACHE_ROOT:-${HOME:-/tmp}/.cache/bread-ci}"
mkdir -p \
    "${CACHE_ROOT}/cargo-registry" \
    "${CACHE_ROOT}/cargo-git" \
    "${CACHE_ROOT}/target-${PRODUCT}"

docker run --rm \
    --user "$(id -u):$(id -g)" \
    -v "${REPO_ROOT}:/workspace" \
    -v "${CACHE_ROOT}/cargo-registry:/cargo-home/registry" \
    -v "${CACHE_ROOT}/cargo-git:/cargo-home/git" \
    -v "${CACHE_ROOT}/target-${PRODUCT}:/cargo-target" \
    -w /workspace \
    -e HOME=/tmp \
    -e CARGO_HOME=/cargo-home \
    -e CARGO_TARGET_DIR=/cargo-target \
    "bread-ci:${PRODUCT}" \
    bash -c '
        set -euo pipefail
        "$@"
        mkdir -p /workspace/target
        cp -a /cargo-target/. /workspace/target/
    ' bash "$@"
