#!/usr/bin/env bash
# Exercise ci/build.sh's host lock with a fake Docker command. Same-product
# invocations must not overlap, while different products must be able to run
# at the same time.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "${TEST_ROOT}"' EXIT

mkdir -p "${TEST_ROOT}/bin" "${TEST_ROOT}/home" "${TEST_ROOT}/repo" "${TEST_ROOT}/active"
cat >"${TEST_ROOT}/bin/docker" <<'MOCK_DOCKER'
#!/usr/bin/env bash
set -euo pipefail

[[ "$1" == run ]] || exit 0
product=""
for arg in "$@"; do
    case "${arg}" in
        bread-ci:*) product="${arg#bread-ci:}" ;;
    esac
done
[[ -n "${product}" ]]

active="${BREAD_CI_TEST_ACTIVE}/${product}.active"
if ! mkdir "${active}" 2>/dev/null; then
    touch "${BREAD_CI_TEST_COLLISION}"
    exit 1
fi
trap 'rmdir "${active}"' EXIT

if [[ "${BREAD_CI_TEST_WAIT_FOR_PEER:-0}" == 1 ]]; then
    for _ in {1..100}; do
        active_count="$(find "${BREAD_CI_TEST_ACTIVE}" -mindepth 1 -maxdepth 1 -type d -name '*.active' | wc -l)"
        if (( active_count >= 2 )); then
            touch "${BREAD_CI_TEST_PARALLEL}"
            break
        fi
        sleep 0.02
    done
fi
sleep 0.3
MOCK_DOCKER
chmod +x "${TEST_ROOT}/bin/docker"

run_build() {
    local product="$1"
    shift
    PATH="${TEST_ROOT}/bin:${PATH}" \
        HOME="${TEST_ROOT}/home" \
        BREAD_CI_CACHE_ROOT="${TEST_ROOT}/cache" \
        BREAD_CI_TEST_ACTIVE="${TEST_ROOT}/active" \
        BREAD_CI_TEST_COLLISION="${TEST_ROOT}/collision" \
        BREAD_CI_TEST_PARALLEL="${TEST_ROOT}/parallel" \
        BREAD_CI_TEST_WAIT_FOR_PEER="${BREAD_CI_TEST_WAIT_FOR_PEER:-0}" \
        bash "${SCRIPT_DIR}/build.sh" "${product}" "${TEST_ROOT}/repo" cargo "$@"
}

run_build bread-test same-product-a &
first_pid=$!
run_build bread-test same-product-b &
second_pid=$!
wait "${first_pid}"
wait "${second_pid}"
if [[ -e "${TEST_ROOT}/collision" ]]; then
    echo "same-product builds overlapped" >&2
    exit 1
fi

BREAD_CI_TEST_WAIT_FOR_PEER=1 run_build bread-one different-product-a &
first_pid=$!
BREAD_CI_TEST_WAIT_FOR_PEER=1 run_build bread-two different-product-b &
second_pid=$!
wait "${first_pid}"
wait "${second_pid}"
if [[ ! -e "${TEST_ROOT}/parallel" ]]; then
    echo "different-product builds did not overlap" >&2
    exit 1
fi

echo "build.sh serializes same-product builds and permits cross-product concurrency"
