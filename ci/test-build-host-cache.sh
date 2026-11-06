#!/usr/bin/env bash
# End-to-end checks for the CI host cache wiring without Docker or a real mount.
set -euo pipefail

CI_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT

mkdir -p "${TMP}/bin" "${TMP}/repo" "${TMP}/home"
: >"${TMP}/docker.log"

cat >"${TMP}/bin/docker" <<'EOF'
#!/usr/bin/env bash
echo "docker $*" >>"${FAKE_DOCKER_LOG}"
exit 0
EOF
chmod +x "${TMP}/bin/docker"

cat >"${TMP}/bin/findmnt" <<'EOF'
#!/usr/bin/env bash
echo "test-uuid"
EOF
chmod +x "${TMP}/bin/findmnt"

export PATH="${TMP}/bin:${PATH}"
export HOME="${TMP}/home"
export FAKE_DOCKER_LOG="${TMP}/docker.log"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

# Legacy mode still uses the per-product directory under BREAD_CI_CACHE_ROOT.
BREAD_CI_CACHE_ROOT="${TMP}/cache" "${CI_DIR}/build.sh" demo "${TMP}/repo" true
[[ -d "${TMP}/cache/target-demo" ]] || fail "legacy target directory missing"

# Host mode writes targets and a lease record under the selected root.
BREAD_CI_CACHE_ROOT="${TMP}/cache2" \
BREAD_CI_HOST_CACHE="${TMP}/host" \
BREAD_CI_HOST_CACHE_UUID="test-uuid" \
    "${CI_DIR}/build.sh" demo "${TMP}/repo" true

KEY="$(cd "${CI_DIR}" && python3 host_cache.py key demo)"
[[ -d "${TMP}/host/targets/${KEY}" ]] || fail "host target directory missing"
[[ -f "${TMP}/host/state/${KEY}.json" ]] || fail "lease record missing"
grep -q '"target":' "${TMP}/host/state/${KEY}.json" || fail "lease record shape"

# A wrong filesystem UUID must refuse before docker runs.
if BREAD_CI_CACHE_ROOT="${TMP}/cache3" \
   BREAD_CI_HOST_CACHE="${TMP}/host" \
   BREAD_CI_HOST_CACHE_UUID="wrong-uuid" \
   "${CI_DIR}/build.sh" demo "${TMP}/repo" true >/dev/null 2>&1; then
    fail "wrong UUID was accepted"
fi

# An unset UUID must refuse too.
if BREAD_CI_CACHE_ROOT="${TMP}/cache4" \
   BREAD_CI_HOST_CACHE="${TMP}/host" \
   "${CI_DIR}/build.sh" demo "${TMP}/repo" true >/dev/null 2>&1; then
    fail "unset UUID was accepted"
fi

echo "PASS: legacy and host cache wiring, lease record and fail-closed UUID checks"
