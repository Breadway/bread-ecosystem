#!/usr/bin/env bash
# Verify that ci/build.sh only copies the per-product cargo target back into
# the checkout for release builds. Copies of 20+ GiB after every check/test
# step dominated CI wall time (ECO-110).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "${TEST_ROOT}"' EXIT
mkdir -p "${TEST_ROOT}/repo"

expect_copyback() {
    local expected="$1"
    shift
    local out
    out="$(BREAD_CI_ECHO=1 bash "${SCRIPT_DIR}/build.sh" bread-test "${TEST_ROOT}/repo" "$@")"
    if [[ "${out}" != "copyback=${expected}" ]]; then
        echo "FAIL: $* -> ${out}, expected copyback=${expected}" >&2
        exit 1
    fi
}

expect_copyback 0 cargo clippy --workspace --all-targets --locked -- -D warnings
expect_copyback 0 cargo test --workspace --locked
expect_copyback 0 cargo check --workspace
expect_copyback 0 cargo fmt --all -- --check
expect_copyback 0 cargo build --workspace
expect_copyback 1 cargo build --release --locked -p breadpad -p breadman
expect_copyback 1 cargo build --release

out="$(BREAD_CI_FORCE_COPYBACK=1 BREAD_CI_ECHO=1 bash "${SCRIPT_DIR}/build.sh" bread-test "${TEST_ROOT}/repo" cargo clippy)"
[[ "${out}" == "copyback=1" ]] || { echo "FAIL: force override -> ${out}" >&2; exit 1; }

out="$(BREAD_CI_SKIP_COPYBACK=1 BREAD_CI_ECHO=1 bash "${SCRIPT_DIR}/build.sh" bread-test "${TEST_ROOT}/repo" cargo build --release)"
[[ "${out}" == "copyback=0" ]] || { echo "FAIL: skip override -> ${out}" >&2; exit 1; }

echo "build.sh copies the target back only for release builds"
