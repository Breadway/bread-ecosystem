#!/usr/bin/env bash
# Publish a built package to dl.breadway.dev from a GitHub-hosted runner.
#
# The runner has no /srv/breadway-dl tree and is not on the tailnet, so it
# regenerates the one package entry against the current published index
# (INDEX_MERGE_BASE), signs the result with the bakery key, and POSTs the
# version directory plus index and signature to the authenticated receiver
# on Hestia. The receiver verifies the HMAC, the signature and every
# artifact hash before installing anything.
#
# Usage:
#   publish-dl.sh --package NAME --version VERSION [--track stable|beta|dev] \
#                 --stage-dir DIR
#
# Required environment:
#   DL_SYNC_HMAC_KEY     shared secret for the receiver
#   MINISIGN_SEC_KEY     path to the bakery minisign secret key
# Optional:
#   DL_BASE              default https://dl.breadway.dev
#   DL_SYNC_URL          default ${DL_BASE}/hooks/dl-sync
#   MINISIGN_SEC_KEY_PASSWORD
set -euo pipefail

PACKAGE=""
VERSION=""
TRACK="stable"
STAGE_DIR=""

while [ $# -gt 0 ]; do
    case "$1" in
        --package) PACKAGE="$2"; shift 2 ;;
        --version) VERSION="$2"; shift 2 ;;
        --track) TRACK="$2"; shift 2 ;;
        --stage-dir) STAGE_DIR="$2"; shift 2 ;;
        *) echo "publish-dl.sh: unknown argument $1" >&2; exit 1 ;;
    esac
done

if [ -z "${PACKAGE}" ] || [ -z "${VERSION}" ] || [ -z "${STAGE_DIR}" ]; then
    echo "usage: publish-dl.sh --package NAME --version VERSION --stage-dir DIR [--track stable|beta|dev]" >&2
    exit 1
fi
case "${TRACK}" in stable|beta|dev) ;; *) echo "publish-dl.sh: bad track ${TRACK}" >&2; exit 1 ;; esac
case "${PACKAGE}" in *[!a-z0-9-]*|"") echo "publish-dl.sh: bad package ${PACKAGE}" >&2; exit 1 ;; esac
case "${VERSION}" in *[!0-9A-Za-z.+-]*|"") echo "publish-dl.sh: bad version ${VERSION}" >&2; exit 1 ;; esac
[ -d "${STAGE_DIR}" ] || { echo "publish-dl.sh: stage dir ${STAGE_DIR} is missing" >&2; exit 1; }
[ -n "${DL_SYNC_HMAC_KEY:-}" ] || { echo "publish-dl.sh: DL_SYNC_HMAC_KEY is not set" >&2; exit 1; }
[ -n "${MINISIGN_SEC_KEY:-}" ] || { echo "publish-dl.sh: MINISIGN_SEC_KEY is not set" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DL_BASE="${DL_BASE:-https://dl.breadway.dev}"
DL_SYNC_URL="${DL_SYNC_URL:-${DL_BASE}/hooks/dl-sync}"
if [ "${TRACK}" = "stable" ]; then
    URL_ROOT="${DL_BASE}"
else
    URL_ROOT="${DL_BASE}/${TRACK}"
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/publish-dl-XXXXXX")"
trap 'rm -rf -- "${WORK}"' EXIT
TREE="${WORK}/dl"
if [ "${TRACK}" = "stable" ]; then
    TRACK_ROOT="${TREE}"
else
    TRACK_ROOT="${TREE}/${TRACK}"
fi
mkdir -p "${TRACK_ROOT}/${PACKAGE}/${VERSION}"
cp -a "${STAGE_DIR}/." "${TRACK_ROOT}/${PACKAGE}/${VERSION}/"
ln -sfn "${VERSION}" "${TRACK_ROOT}/${PACKAGE}/latest"

sign_and_post() {
    DL_DIR="${TREE}" TRACK="${TRACK}" INDEX_MERGE_BASE="${WORK}/base.json" \
        MINISIGN_SEC_KEY="${MINISIGN_SEC_KEY}" \
        MINISIGN_SEC_KEY_PASSWORD="${MINISIGN_SEC_KEY_PASSWORD:-}" \
        bash "${SCRIPT_DIR}/scripts/gen-index.sh"
    INDEX="${TRACK_ROOT}/index.json"
    [ -f "${INDEX}.minisig" ] || { echo "publish-dl.sh: index was not signed" >&2; exit 1; }
    if ! jq -e --arg p "${PACKAGE}" --arg v "${VERSION}" \
        '.packages[$p].version == $v' "${INDEX}" >/dev/null; then
        echo "publish-dl.sh: ${PACKAGE} ${VERSION} is missing from the generated index; is it registered in registry/bread-ecosystem.toml?" >&2
        exit 1
    fi
    PAYLOAD="${WORK}/payload.tar.gz"
    tar -czf "${PAYLOAD}" -C "${TRACK_ROOT}" index.json index.json.minisig \
        "${PACKAGE}/${VERSION}"
    local ts query body_sha signature code
    ts="$(date +%s)"
    query="track=${TRACK}&package=${PACKAGE}&version=${VERSION}"
    body_sha="$(sha256sum "${PAYLOAD}" | awk '{print $1}')"
    signature="$(TS="${ts}" QUERY="${query}" BODY_SHA="${body_sha}" \
        DL_SYNC_HMAC_KEY="${DL_SYNC_HMAC_KEY}" python3 - <<'PY'
import hashlib, hmac, os
message = f"{os.environ['TS']}\nPOST\n/hooks/dl-sync\n{os.environ['QUERY']}\n{os.environ['BODY_SHA']}".encode()
print(hmac.new(os.environ['DL_SYNC_HMAC_KEY'].encode(), message, hashlib.sha256).hexdigest())
PY
)"
    code="$(curl -sS --max-time 300 -o "${WORK}/response.json" -w '%{http_code}' \
        -X POST "${DL_SYNC_URL}?${query}" \
        -H "Content-Type: application/octet-stream" \
        -H "X-Breadway-Signature: t=${ts},v1=${signature}" \
        --data-binary @"${PAYLOAD}" || true)"
    echo "publish-dl.sh: receiver returned ${code}: $(cat "${WORK}/response.json" 2>/dev/null || true)" >&2
    return "${code}"
}

for attempt in 1 2 3; do
    if ! curl -fsS --max-time 60 "${URL_ROOT}/index.json" -o "${WORK}/base.json"; then
        echo "publish-dl.sh: could not fetch the current ${TRACK} index, refusing to publish a partial index" >&2
        exit 1
    fi
    set +e
    sign_and_post
    code=$?
    set -e
    if [ "${code}" = "200" ]; then
        cat "${WORK}/response.json"
        exit 0
    fi
    if [ "${code}" = "409" ] && [ "${attempt}" -lt 3 ]; then
        echo "publish-dl.sh: another release landed first, re-merging (attempt ${attempt})" >&2
        sleep $((attempt * 3))
        continue
    fi
    exit 1
done
