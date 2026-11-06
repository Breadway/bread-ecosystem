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
# <product-name> is used verbatim as the image tag and cache-volume name;
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
# mounts, not named docker volumes, see the --user note below) across runs.
set -euo pipefail

if [ $# -lt 3 ]; then
    echo "usage: build.sh <product-name> <product-repo-root> <cargo-command...>" >&2
    exit 1
fi

PRODUCT="$1"
REPO_ROOT="$(cd "$2" && pwd)"
shift 2

# The container mounts REPO_ROOT's PARENT, not REPO_ROOT itself, and runs
# with its cwd set to REPO_ROOT's own subdirectory inside that mount (see
# -w below). This matters for scripts/dev-deps.sh (bread-ecosystem;
# ECO-38): it clones each git.breadway.dev dependency as a true SIBLING of
# the checkout (its default DEV_DEPS_DIR), then writes a `.cargo/config.toml`
# `[patch]` block with relative paths like `../bread-ecosystem/bread-theme`.
# A sibling clone that's a filesystem descendant of the checkout's own
# workspace root (the old workaround: DEV_DEPS_DIR forced *inside* the
# checkout so a REPO_ROOT-only mount could see it) breaks Cargo's
# workspace-field inheritance (`edition.workspace = true` and friends):
# Cargo attributes the nested clone to the OUTER (checkout's) workspace
# instead of discovering the clone's own nested `[workspace]` root, and
# fails with "workspace.package.edition was not defined" the moment the
# checkout is itself a multi-crate workspace (breadbox, breadclip,
# breadpad, breadsearch, breadarr, breadcast). Mounting the parent
# directory instead keeps the sibling clone a genuine, non-nested sibling
# on both sides of the bind mount, exactly like a developer's own
# sibling checkout on a workstation (see CONTRIBUTING.md).
REPO_PARENT="$(dirname "${REPO_ROOT}")"
REPO_NAME="$(basename "${REPO_ROOT}")"

CI_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

EXTRA_PKGS=""
if [ -f "${REPO_ROOT}/ci/deps.txt" ]; then
    EXTRA_PKGS="$(grep -vE '^\s*(#|$)' "${REPO_ROOT}/ci/deps.txt" | tr '\n' ' ')"
fi

# Persistent cache directories (see the --user note below for why these are
# host bind mounts). Created before anything else because the per-product
# build lock lives in the same directory.
#
# Two cache layers, deliberately separate:
#   - CACHE_ROOT holds the fetched cargo registry/git caches and the legacy
#     target directories. BREAD_CI_CACHE_ROOT controls it and defaults to
#     $HOME/.cache/bread-ci.
#   - BREAD_CI_HOST_CACHE, when set, is the generated-target store. It must be
#     an owned directory on the filesystem identified by
#     BREAD_CI_HOST_CACHE_UUID; a missing or different mount fails closed
#     instead of writing to the root disk. This is independent of the pinned
#     source checkout cache that product CI may also call BREAD_CI_CACHE_ROOT.
# Do not point the two variables at the same directory.
CACHE_ROOT="${BREAD_CI_CACHE_ROOT:-${HOME:-/tmp}/.cache/bread-ci}"
mkdir -p \
    "${CACHE_ROOT}/cargo-registry" \
    "${CACHE_ROOT}/cargo-git"

HOST_CACHE="${BREAD_CI_HOST_CACHE:-}"
if [ -n "${HOST_CACHE}" ]; then
    if [ -z "${BREAD_CI_HOST_CACHE_UUID:-}" ]; then
        echo "build.sh: BREAD_CI_HOST_CACHE is set but BREAD_CI_HOST_CACHE_UUID is not; refusing an unverified cache root" >&2
        exit 1
    fi
    python3 "${CI_DIR}/host_cache.py" check --root "${HOST_CACHE}" --uuid "${BREAD_CI_HOST_CACHE_UUID}"
    HOST_KEY="$(python3 "${CI_DIR}/host_cache.py" key "${PRODUCT}")"
    TARGET_DIR="${HOST_CACHE}/targets/${HOST_KEY}"
    LOCK_FILE="${HOST_CACHE}/state/${HOST_KEY}.lock"
    mkdir -p "${TARGET_DIR}" "$(dirname "${LOCK_FILE}")"
else
    HOST_KEY=""
    TARGET_DIR="${CACHE_ROOT}/target-${PRODUCT}"
    LOCK_FILE="${TARGET_DIR}.lock"
    mkdir -p "${TARGET_DIR}"
fi

# Serialize builds per product. Two jobs for the same product (a dev-release
# and a check, say) share one target directory: while one job's cargo is
# rewriting `incremental/`, the other's final `cp -a /cargo-target/. target/`
# fails with "cannot stat '/cargo-target/./debug/incremental/...'" (seen on
# breadcast). The lock is held until this script exits, since fd 9 stays open
# through the docker build and run. Different products never contend, and
# neither do repos that build on different runners.
#
# Under BREAD_CI_HOST_CACHE the same lock file is the retention lease. The
# cleanup helper checks it with flock -n, so an active build can never be
# deleted, and the record below is refreshed after the artifact copyback.
if ! command -v flock >/dev/null 2>&1; then
    echo "build.sh: flock is required to protect the shared ${PRODUCT} target cache" >&2
    exit 1
fi
exec 9>"${LOCK_FILE}"
if ! flock -n 9; then
    echo "build.sh: another ${PRODUCT} build is running; waiting for its lock..." >&2
    flock -w "${BREAD_CI_LOCK_WAIT:-3600}" 9 || {
        echo "build.sh: timed out waiting for the ${PRODUCT} build lock" >&2
        exit 1
    }
fi
if [ -n "${HOST_CACHE}" ]; then
    python3 "${CI_DIR}/host_cache.py" touch "${HOST_CACHE}" "${HOST_KEY}" "${TARGET_DIR}"
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
# (CACHE_ROOT and its directories are set up above, before the build lock.
#  TARGET_DIR is either the legacy per-product directory under CACHE_ROOT or
#  the lease-managed directory under BREAD_CI_HOST_CACHE.)

# The host's /etc/passwd and /etc/group are mounted read-only so the numeric
# --user below resolves to a real account inside the container. Without an
# entry, anything that looks up the current user fails: dbus-daemon refuses
# every connection ("Could not get password database information for UID of
# current process"), which broke breadcrumbs' fake-NetworkManager tests.
docker run --rm \
    --user "$(id -u):$(id -g)" \
    -v /etc/passwd:/etc/passwd:ro \
    -v /etc/group:/etc/group:ro \
    -v "${REPO_PARENT}:/workspace" \
    -v "${CACHE_ROOT}/cargo-registry:/cargo-home/registry" \
    -v "${CACHE_ROOT}/cargo-git:/cargo-home/git" \
    -v "${TARGET_DIR}:/cargo-target" \
    -w "/workspace/${REPO_NAME}" \
    -e HOME=/tmp \
    -e CARGO_HOME=/cargo-home \
    -e CARGO_TARGET_DIR=/cargo-target \
    "bread-ci:${PRODUCT}" \
    bash -c '
        set -euo pipefail
        "$@"
        mkdir -p "/workspace/'"${REPO_NAME}"'/target"
        cp -a /cargo-target/. "/workspace/'"${REPO_NAME}"'/target/"
    ' bash "$@"

# Keep the lease fresh through the artifact copyback above. A release workflow
# that copies repo/target/release therefore still sees the same path, while
# retention treats the off-root target as in use until this point.
if [ -n "${HOST_CACHE}" ]; then
    python3 "${CI_DIR}/host_cache.py" touch "${HOST_CACHE}" "${HOST_KEY}" "${TARGET_DIR}"
fi
