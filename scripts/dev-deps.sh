#!/usr/bin/env bash
# dev-deps.sh: resolve an app's git.breadway.dev dependencies to their
# `main` branch for a dev-track build, without touching the app's
# Cargo.toml tag pins.
#
# Cargo.toml always pins git.breadway.dev crates to a release tag (see
# docs/release-channels.md). For a dev-track build or a PR/branch check we
# instead want each dependency resolved to its own `main`, so a cross-repo
# change (e.g. bread-theme in bread-ecosystem + a consumer in breadbar) can
# be exercised together before either side is tagged. This script:
#
#   1. Finds every dependency declared across the app's Cargo.toml manifests
#      (workspace root and every member) whose source is git.breadway.dev
#      (any repo, any number of crates from that repo). Parsed straight
#      from the manifests, not from a resolved dependency graph, so this is
#      idempotent even after a previous run has already patched things.
#   2. Shallow-clones each distinct dependency repo's `main` into a sibling
#      directory (default "../<repo-name>"; see DEV_DEPS_DIR below).
#   3. Maps each dependency crate name to its manifest directory inside
#      that clone (via `cargo metadata` on the clone itself, so multi-crate
#      repos and workspace layouts resolve correctly).
#   4. Writes a CI-owned `.cargo/config.toml` at the app root with a
#      `[patch]` block per repo, pointing each crate at its clone's path.
#   5. Runs `cargo update -p <crate>` for every patched crate so
#      Cargo.lock matches the new resolution.
#   6. Prints and writes a dependency-SHA record (default
#      "dev-deps.lock.txt") for reproducibility.
#
# Usage:
#   scripts/dev-deps.sh [--check] [--force] [--root <app-root>]
#
#   --check    Dry run: discover dependencies and print the plan (clone
#              dirs, owned/reused/refreshed) without touching the
#              filesystem, .cargo/config.toml, or Cargo.lock. Exit status
#              is still 0 on success, 1 if the plan itself can't be
#              determined (e.g. a manifest fails to parse).
#   --force    Overwrite an existing .cargo/config.toml that this script
#              did not generate. Without --force, a foreign config.toml
#              (e.g. a developer's local path-patch setup) is left alone
#              and the script exits with an error.
#   --root     App checkout root to operate on (default: current directory).
#
# Env vars:
#   DEV_DEPS_DIR   Parent directory under which each dependency repo is
#                  cloned as "<DEV_DEPS_DIR>/<repo-name>". Default is the
#                  app root's own parent directory (a true sibling
#                  checkout, matching local dev convention). CI overrides
#                  this to a directory *inside* the app checkout (e.g.
#                  "$PWD/.dev-deps") when the build itself runs inside a
#                  container that only bind-mounts the checkout root; see
#                  breadbar's dev-release.yml / check.yml for the concrete
#                  case this exists for.
#   DEV_DEPS_HOST  Git host to match dependencies against. Default
#                  "git.breadway.dev". Only useful for testing.
#   DEV_DEPS_BRANCH Branch to resolve dependencies to. Default "main".
#
# A clone this script creates is marked with a ".dev-deps-owned" file
# inside it. On a later run, an owned clone is refreshed in place
# (`git fetch --depth 1 origin <branch> && git reset --hard FETCH_HEAD`);
# a directory that already existed without that marker (a developer's own
# checkout, e.g. via the .cargo/config.toml convention described in
# CONTRIBUTING.md) is reused as-is and never fetched, reset, or otherwise
# mutated.
#
# Requires: git, cargo, python3 (tomllib for Cargo.toml, the stdlib json
# module for cargo metadata's output when mapping crates to clone paths).
set -euo pipefail

CHECK=0
FORCE=0
APP_ROOT="."

while [ $# -gt 0 ]; do
    case "$1" in
        --check|--dry-run)
            CHECK=1
            shift
            ;;
        --force)
            FORCE=1
            shift
            ;;
        --root)
            APP_ROOT="$2"
            shift 2
            ;;
        -h|--help)
            sed -n '2,60p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

APP_ROOT="$(cd "${APP_ROOT}" && pwd)"
DEV_DEPS_HOST="${DEV_DEPS_HOST:-git.breadway.dev}"
DEV_DEPS_BRANCH="${DEV_DEPS_BRANCH:-main}"
DEV_DEPS_DIR="${DEV_DEPS_DIR:-$(cd "${APP_ROOT}/.." && pwd)}"
mkdir -p "${DEV_DEPS_DIR}"
DEV_DEPS_DIR="$(cd "${DEV_DEPS_DIR}" && pwd)"

LOCK_FILE="${DEV_DEPS_LOCK_FILE:-${APP_ROOT}/dev-deps.lock.txt}"
CONFIG_FILE="${APP_ROOT}/.cargo/config.toml"
OWNED_MARKER=".dev-deps-owned"
CONFIG_SENTINEL="# generated-by: bread-ecosystem/scripts/dev-deps.sh"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

# Retries a network-facing git command up to 3 times with a short backoff.
# The self-hosted runner's own clone of a dependency repo (this script's
# whole purpose) is a single point of failure for every dev-track build and
# PR check across the ecosystem; a transient DNS blip or connection reset
# shouldn't fail the job outright when a retry would succeed.
git_retry() {
    local attempt=1 max=3 delay=3
    until "$@"; do
        if [ "${attempt}" -ge "${max}" ]; then
            echo "::error::git command failed after ${max} attempts: $*" >&2
            return 1
        fi
        echo "==> dev-deps.sh: git command failed (attempt ${attempt}/${max}), retrying in ${delay}s: $*" >&2
        sleep "${delay}"
        attempt=$((attempt + 1))
        delay=$((delay * 2))
    done
}

echo "==> dev-deps.sh: app root ${APP_ROOT}"
echo "==> dev-deps.sh: cloning ${DEV_DEPS_HOST} deps' ${DEV_DEPS_BRANCH} into ${DEV_DEPS_DIR}"

# --- 1. Discover git.breadway.dev dependencies ------------------------------
#
# Parsed directly from every Cargo.toml under the app root (workspace root
# plus all members), not from `cargo metadata`'s resolved graph: once this
# script has run once, the patched crates are path dependencies and drop
# their `source` entirely from metadata, which would make discovery on a
# second run (e.g. refreshing an already-cloned dependency) find nothing.
# Cargo.toml's declared `git = "..."` is unaffected by any [patch], so
# parsing it directly is idempotent regardless of prior runs.
mapfile -t MANIFESTS < <(find "${APP_ROOT}" \
    -name Cargo.toml \
    -not -path '*/target/*' \
    -not -path '*/.dev-deps/*' \
    -not -path '*/.git/*' \
    | sort)

DEPS_TSV="$(python3 - "${DEV_DEPS_HOST}" "${MANIFESTS[@]}" <<'PYEOF'
import sys, tomllib

host = sys.argv[1]
manifests = sys.argv[2:]

def dep_tables(doc):
    tables = []
    for key in ("dependencies", "dev-dependencies", "build-dependencies"):
        if key in doc:
            tables.append(doc[key])
    for tgt in doc.get("target", {}).values():
        for key in ("dependencies", "dev-dependencies", "build-dependencies"):
            if key in tgt:
                tables.append(tgt[key])
    if "workspace" in doc and "dependencies" in doc["workspace"]:
        tables.append(doc["workspace"]["dependencies"])
    return tables

seen = set()
for manifest in manifests:
    with open(manifest, "rb") as f:
        doc = tomllib.load(f)
    for table in dep_tables(doc):
        for name, spec in table.items():
            if not isinstance(spec, dict):
                continue
            git = spec.get("git", "")
            if not isinstance(git, str) or host not in git:
                continue
            repo_path = git.split("://", 1)[-1].split("/", 1)[-1]
            repo_path = repo_path.split("?", 1)[0].split("#", 1)[0]
            if repo_path.endswith(".git"):
                repo_path = repo_path[: -len(".git")]
            repo_path = repo_path.strip("/")
            repo_url = f"https://{host}/{repo_path}"
            key = (repo_url, name)
            if key in seen:
                continue
            seen.add(key)
            print(f"{repo_url}\t{repo_path.rsplit('/', 1)[-1]}\t{name}")
PYEOF
)"

if [ -z "${DEPS_TSV}" ]; then
    echo "==> dev-deps.sh: no ${DEV_DEPS_HOST} dependencies found; nothing to do"
    exit 0
fi

echo "==> dev-deps.sh: found dependencies:"
echo "${DEPS_TSV}" | awk -F'\t' '{printf "    %s (%s) via %s\n", $3, $1, $2}'

# Distinct repo URLs, in stable order.
mapfile -t REPO_URLS < <(printf '%s\n' "${DEPS_TSV}" | cut -f1 | sort -u)

: > "${LOCK_FILE}.tmp"

declare -A CLONE_DIR_FOR_URL

for repo_url in "${REPO_URLS[@]}"; do
    repo_name="$(basename "${repo_url}")"
    clone_dir="${DEV_DEPS_DIR}/${repo_name}"
    CLONE_DIR_FOR_URL["${repo_url}"]="${clone_dir}"

    if [ -d "${clone_dir}/.git" ]; then
        if [ -f "${clone_dir}/${OWNED_MARKER}" ]; then
            if [ "${CHECK}" -eq 1 ]; then
                echo "==> dev-deps.sh: [check] would refresh owned clone ${clone_dir}"
            else
                echo "==> dev-deps.sh: refreshing owned clone ${clone_dir}"
                git_retry git -C "${clone_dir}" fetch --depth 1 origin "${DEV_DEPS_BRANCH}"
                git -C "${clone_dir}" reset --hard FETCH_HEAD
            fi
        else
            echo "==> dev-deps.sh: reusing existing checkout ${clone_dir} as-is (not created by this script; never fetched/reset)"
        fi
    else
        if [ -e "${clone_dir}" ]; then
            echo "::error::${clone_dir} exists and is not a git checkout; refusing to clone over it" >&2
            exit 1
        fi
        if [ "${CHECK}" -eq 1 ]; then
            echo "==> dev-deps.sh: [check] would clone ${repo_url}#${DEV_DEPS_BRANCH} into ${clone_dir}"
        else
            echo "==> dev-deps.sh: cloning ${repo_url}#${DEV_DEPS_BRANCH} into ${clone_dir}"
            git_retry git clone --depth 1 --branch "${DEV_DEPS_BRANCH}" "${repo_url}" "${clone_dir}"
            touch "${clone_dir}/${OWNED_MARKER}"
        fi
    fi

    if [ "${CHECK}" -eq 0 ]; then
        sha="$(git -C "${clone_dir}" rev-parse HEAD)"
        echo "${repo_name} ${sha}" >> "${LOCK_FILE}.tmp"
    fi
done

if [ "${CHECK}" -eq 1 ]; then
    echo "==> dev-deps.sh: [check] plan complete, no changes made"
    exit 0
fi

sort -o "${LOCK_FILE}.tmp" "${LOCK_FILE}.tmp"
mv "${LOCK_FILE}.tmp" "${LOCK_FILE}"
echo "==> dev-deps.sh: dependency SHAs recorded in ${LOCK_FILE}:"
sed 's/^/    /' "${LOCK_FILE}"

# --- 2. Map each crate name to its manifest dir inside the clone -----------

declare -A PATCH_ENTRIES_FOR_URL   # repo_url -> newline-separated "crate\tpath"
PATCHED_CRATES=()

while IFS=$'\t' read -r repo_url repo_name crate_name; do
    clone_dir="${CLONE_DIR_FOR_URL["${repo_url}"]}"
    manifest_dir="$(
        cargo metadata --format-version 1 --no-deps \
            --manifest-path "${clone_dir}/Cargo.toml" \
        | python3 -c "
import json, sys
data = json.load(sys.stdin)
name = sys.argv[1]
for pkg in data['packages']:
    if pkg['name'] == name:
        import os
        print(os.path.dirname(pkg['manifest_path']))
        break
" "${crate_name}"
    )"
    if [ -z "${manifest_dir}" ]; then
        echo "::error::could not find crate ${crate_name} inside ${clone_dir} (cloned from ${repo_url}); the crate may have been renamed or moved on ${DEV_DEPS_BRANCH}" >&2
        exit 1
    fi
    # Cargo resolves a relative path in .cargo/config.toml against the
    # directory *containing* .cargo/ (i.e. the project root), not against
    # .cargo/ itself: verified empirically, contrary to a first reading of
    # "the parent directory where config.toml is located".
    rel_path="$(python3 -c "import os,sys; print(os.path.relpath(sys.argv[1], sys.argv[2]))" "${manifest_dir}" "${APP_ROOT}")"
    PATCH_ENTRIES_FOR_URL["${repo_url}"]+="${crate_name}\t${rel_path}\n"
    PATCHED_CRATES+=("${crate_name}")
done <<< "${DEPS_TSV}"

# --- 3. Write .cargo/config.toml (CI-owned; refuse to clobber a foreign one) -

if [ -f "${CONFIG_FILE}" ] && ! head -n1 "${CONFIG_FILE}" | grep -qF "${CONFIG_SENTINEL}"; then
    if [ "${FORCE}" -eq 1 ]; then
        echo "==> dev-deps.sh: ${CONFIG_FILE} exists and wasn't generated by this script; overwriting because --force was passed"
    else
        echo "::error::${CONFIG_FILE} already exists and was not generated by dev-deps.sh (its first line doesn't match \"${CONFIG_SENTINEL}\")." >&2
        echo "::error::This looks like a developer's local path-patch setup (see CONTRIBUTING.md). Refusing to overwrite it." >&2
        echo "::error::Pass --force to overwrite anyway, or remove it yourself if you know it's stale." >&2
        exit 1
    fi
fi

mkdir -p "${APP_ROOT}/.cargo"
{
    echo "${CONFIG_SENTINEL}"
    echo "# Regenerate with scripts/dev-deps.sh; do not edit by hand."
    echo "# See bread-ecosystem/docs/release-channels.md for what this is."
    echo
    for repo_url in "${REPO_URLS[@]}"; do
        echo "[patch.\"${repo_url}\"]"
        printf '%b' "${PATCH_ENTRIES_FOR_URL["${repo_url}"]}" | while IFS=$'\t' read -r crate_name rel_path; do
            [ -z "${crate_name}" ] && continue
            echo "${crate_name} = { path = \"${rel_path}\" }"
        done
        echo
    done
} > "${CONFIG_FILE}"

echo "==> dev-deps.sh: wrote ${CONFIG_FILE}:"
sed 's/^/    /' "${CONFIG_FILE}"

# --- 4. cargo update the patched packages so Cargo.lock matches ------------

UPDATE_ARGS=()
for crate in "${PATCHED_CRATES[@]}"; do
    UPDATE_ARGS+=(-p "${crate}")
done

echo "==> dev-deps.sh: cargo update ${UPDATE_ARGS[*]}"
(cd "${APP_ROOT}" && cargo update "${UPDATE_ARGS[@]}")

echo "==> dev-deps.sh: done. Build without --locked to pick up the patched deps."
