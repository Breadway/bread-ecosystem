#!/usr/bin/env bash
# test-dev-deps.sh: regression tests for scripts/dev-deps.sh, run offline
# against a local git dependency.
#
# Covers the unused-patch failure mode (ECO-98): a `[patch]` entry is only
# applied when the patched version is semver-compatible with the version the
# product manifest requires. A 0.x minor bump is a breaking change, so a
# product pinned to 0.7.x with an explicit `version` requirement keeps
# resolving the tag while Cargo warns:
#
#   warning: patch `bread-utils v0.8.1 (...)` was not used in the crate graph
#
# dev-deps.sh must not exit 0 in that case. It must also update every cargo
# workspace under the product root, not just the root lock, because a nested
# workspace with its own Cargo.lock would otherwise stay tag-pinned.
#
# No network access is needed: git URL rewriting maps the fake
# git.breadway.dev host to a local repository, and the test uses an isolated
# CARGO_HOME.
#
# Usage: bash scripts/test-dev-deps.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${ROOT}/scripts/dev-deps.sh"
HOST="test.breadway.local"
DEP_URL="https://${HOST}/Breadway/dep"
DEP_REPO_NAME="dep"
PINNED_TAG="v0.7.2"
PINNED_VERSION="0.7.2"
PATCHED_VERSION="0.8.1"

for tool in git cargo python3; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "FAIL: ${tool} is required to run this test" >&2
        exit 1
    fi
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/eco-98-dev-deps-test.XXXXXX")"
cleanup() {
    rm -rf "${WORK}"
}
trap cleanup EXIT

export GIT_CONFIG_GLOBAL="${WORK}/gitconfig"
export GIT_CONFIG_NOSYSTEM=1
export CARGO_NET_GIT_FETCH_WITH_CLI=true
export CARGO_HOME="${WORK}/cargo-home"
export CARGO_TARGET_DIR="${WORK}/target"
export DEV_DEPS_HOST="${HOST}"
unset DEV_DEPS_DIR DEV_DEPS_BRANCH DEV_DEPS_LOCK_FILE

mkdir -p "${CARGO_HOME}"

# Map the fake dependency host onto a local directory so git (and Cargo's
# git CLI backend) never touches the network.
cat >"${GIT_CONFIG_GLOBAL}" <<EOF
[url "file://${WORK}/host/"]
    insteadOf = https://${HOST}/
EOF

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

pass() {
    echo "PASS: $*"
}

# make_dep_repo: one local git repo holding bread-utils v0.7.2 at tag
# v0.7.2 and v0.8.1 on main.
make_dep_repo() {
    local dep_dir="${WORK}/host/Breadway/dep"
    mkdir -p "${dep_dir}/bread-utils/src"
    printf '%s\n' '[workspace]' 'members = ["bread-utils"]' 'resolver = "2"' >"${dep_dir}/Cargo.toml"
    cat >"${dep_dir}/bread-utils/Cargo.toml" <<EOF
[package]
name = "bread-utils"
version = "${PINNED_VERSION}"
edition = "2021"
EOF
    echo 'pub fn util() -> &'"'"'static str { "0.7.2-tag" }' >"${dep_dir}/bread-utils/src/lib.rs"
    (
        cd "${dep_dir}"
        git init -q
        git checkout -q -b main
        git add -A
        git -c user.email=test@example.invalid -c user.name=test commit -qm "${PINNED_TAG}"
        git tag "${PINNED_TAG}"
        sed -i "s/version = \"${PINNED_VERSION}\"/version = \"${PATCHED_VERSION}\"/" bread-utils/Cargo.toml
        echo 'pub fn util() -> &'"'"'static str { "0.8.1-main" }' >bread-utils/src/lib.rs
        git -c user.email=test@example.invalid -c user.name=test commit -qam "main ${PATCHED_VERSION}"
    )
}

# write_product <dir> <dependency-line>
write_product() {
    local dir="$1"
    local dep_line="$2"
    mkdir -p "${dir}/src"
    cat >"${dir}/Cargo.toml" <<EOF
[package]
name = "product"
version = "0.1.0"
edition = "2021"

[dependencies]
bread-utils = ${dep_line}
EOF
    echo 'fn main() { println!("{}", bread_utils::util()); }' >"${dir}/src/main.rs"
}

generate_lock() {
    local dir="$1"
    (cd "${dir}" && cargo generate-lockfile >/dev/null 2>&1) \
        || fail "could not generate a lockfile in ${dir}"
}

# lock_package <Cargo.lock> <crate>: prints "<version> <source>" where source
# is "path" for a patched (path) package and the git source otherwise.
lock_package() {
    python3 - "$1" "$2" <<'PYEOF'
import sys
import tomllib

lock_file, name = sys.argv[1:3]
with open(lock_file, "rb") as f:
    doc = tomllib.load(f)
for pkg in doc.get("package", []):
    if pkg.get("name") == name:
        print(pkg.get("version", ""), pkg.get("source", "path"))
        sys.exit(0)
print("missing")
PYEOF
}

assert_lock_is_patched() {
    local app="$1"
    local label="$2"
    local version source
    read -r version source <<<"$(lock_package "${app}/Cargo.lock" bread-utils)"
    [ "${version}" = "${PATCHED_VERSION}" ] \
        || fail "${label}: expected bread-utils ${PATCHED_VERSION} in ${app}/Cargo.lock, got ${version}"
    [ "${source}" = "path" ] \
        || fail "${label}: expected bread-utils to resolve to the path patch, got source ${source}"
}

assert_lock_is_pinned() {
    local app="$1"
    local label="$2"
    local version source
    read -r version source <<<"$(lock_package "${app}/Cargo.lock" bread-utils)"
    [ "${version}" = "${PINNED_VERSION}" ] \
        || fail "${label}: expected bread-utils to stay pinned at ${PINNED_VERSION}, got ${version}"
    case "${source}" in
        git+*) ;;
        *) fail "${label}: expected a git source for the pinned ${PINNED_VERSION}, got ${source}" ;;
    esac
}

run_dev_deps() {
    local app="$1"
    local log="$2"
    local rc
    set +e
    bash "${SCRIPT}" --root "${app}" >"${log}" 2>&1
    rc=$?
    set -e
    echo "${rc}"
}

make_dep_repo

# --- 1. A bare tag pin is semver-compatible with the patch: update applies -
CASE1="${WORK}/case1"
write_product "${CASE1}" "{ git = \"${DEP_URL}\", tag = \"${PINNED_TAG}\" }"
generate_lock "${CASE1}"
assert_lock_is_pinned "${CASE1}" "case1 before"
CASE1_RC="$(run_dev_deps "${CASE1}" "${WORK}/case1.log")"
[ "${CASE1_RC}" -eq 0 ] || fail "case1: dev-deps.sh exited ${CASE1_RC}: $(cat "${WORK}/case1.log")"
if grep -qF 'was not used in the crate graph' "${WORK}/case1.log"; then
    fail "case1: Cargo reported an unused patch: $(cat "${WORK}/case1.log")"
fi
assert_lock_is_patched "${CASE1}" "case1 after"
CASE1_OUT="$(cd "${CASE1}" && cargo run -q 2>/dev/null)"
[ "${CASE1_OUT}" = "0.8.1-main" ] \
    || fail "case1: build ran ${CASE1_OUT} instead of the patched main"
pass "bare tag pin: cargo update applies the patch and the build uses main (${CASE1_OUT})"

# --- 2. Explicit version requirement one 0.x minor behind: fail loudly ------
CASE2="${WORK}/case2"
write_product "${CASE2}" "{ git = \"${DEP_URL}\", tag = \"${PINNED_TAG}\", version = \"${PINNED_VERSION}\" }"
generate_lock "${CASE2}"
assert_lock_is_pinned "${CASE2}" "case2 before"
CASE2_RC="$(run_dev_deps "${CASE2}" "${WORK}/case2.log")"
[ "${CASE2_RC}" -ne 0 ] \
    || fail "case2: dev-deps.sh exited 0 despite an unused patch: $(cat "${WORK}/case2.log")"
grep -qF 'was not used in the crate graph' "${WORK}/case2.log" \
    || fail "case2: Cargo's unused-patch warning was swallowed: $(cat "${WORK}/case2.log")"
grep -qF 'bread-utils' "${WORK}/case2.log" \
    || fail "case2: error does not name the crate: $(cat "${WORK}/case2.log")"
grep -qF "${PATCHED_VERSION}" "${WORK}/case2.log" \
    || fail "case2: error does not name the patched version: $(cat "${WORK}/case2.log")"
grep -qF "${PINNED_TAG}" "${WORK}/case2.log" \
    || fail "case2: error does not name the pinned tag: $(cat "${WORK}/case2.log")"
grep -qF 'refusing to continue' "${WORK}/case2.log" \
    || fail "case2: error does not refuse the unpatched tree: $(cat "${WORK}/case2.log")"
if grep -qF 'dev-deps.sh: done.' "${WORK}/case2.log"; then
    fail "case2: script claimed success after an unused patch"
fi
assert_lock_is_pinned "${CASE2}" "case2 after"
pass "explicit 0.x version requirement: script fails loudly with crate, tag and patched version"

# --- 3. Nested workspace lock is updated too --------------------------------
CASE3="${WORK}/case3"
write_product "${CASE3}" "{ git = \"${DEP_URL}\", tag = \"${PINNED_TAG}\" }"
write_product "${CASE3}/crates/nested" "{ git = \"${DEP_URL}\", tag = \"${PINNED_TAG}\" }"
generate_lock "${CASE3}"
generate_lock "${CASE3}/crates/nested"
assert_lock_is_pinned "${CASE3}" "case3 root before"
assert_lock_is_pinned "${CASE3}/crates/nested" "case3 nested before"
CASE3_RC="$(run_dev_deps "${CASE3}" "${WORK}/case3.log")"
[ "${CASE3_RC}" -eq 0 ] || fail "case3: dev-deps.sh exited ${CASE3_RC}: $(cat "${WORK}/case3.log")"
assert_lock_is_patched "${CASE3}" "case3 root after"
assert_lock_is_patched "${CASE3}/crates/nested" "case3 nested after"
grep -qF "crates/nested" "${WORK}/case3.log" \
    || fail "case3: nested workspace was not updated: $(cat "${WORK}/case3.log")"
pass "nested workspace: both the root and crates/nested locks move to the patch"

# --- 4. Nested workspace with an incompatible pin fails before the build ----
CASE4="${WORK}/case4"
write_product "${CASE4}" "{ git = \"${DEP_URL}\", tag = \"${PINNED_TAG}\" }"
write_product "${CASE4}/crates/nested" "{ git = \"${DEP_URL}\", tag = \"${PINNED_TAG}\", version = \"${PINNED_VERSION}\" }"
generate_lock "${CASE4}"
generate_lock "${CASE4}/crates/nested"
CASE4_RC="$(run_dev_deps "${CASE4}" "${WORK}/case4.log")"
[ "${CASE4_RC}" -ne 0 ] \
    || fail "case4: dev-deps.sh exited 0 despite a nested unused patch: $(cat "${WORK}/case4.log")"
grep -qF 'was not used in the crate graph' "${WORK}/case4.log" \
    || fail "case4: Cargo's nested unused-patch warning was swallowed: $(cat "${WORK}/case4.log")"
grep -qF "crates/nested/Cargo.lock" "${WORK}/case4.log" \
    || fail "case4: error does not identify the nested lock: $(cat "${WORK}/case4.log")"
assert_lock_is_pinned "${CASE4}/crates/nested" "case4 nested after"
pass "nested workspace with an incompatible pin: script fails loudly and leaves the lock unpromoted"

echo "All dev-deps.sh regression tests passed."
