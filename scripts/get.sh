#!/bin/sh
# Bootstrap script: downloads and installs the `bakery` binary.
# Usage: curl https://breadway.dev/get | sh
# Or:    curl -sSfL https://breadway.dev/get | sh
set -eu

BAKERY_VERSION="${BAKERY_VERSION:-latest}"
BIN_DIR="${BAKERY_BIN_DIR:-$HOME/.local/bin}"

die() { echo "error: $*" >&2; exit 1; }

# Verify platform.
uname -m | grep -q x86_64 || die "bakery only supports x86_64 (got $(uname -m))"
uname -s | grep -q Linux  || die "bakery only supports Linux (got $(uname -s))"

# Build download URLs. GitHub's "latest" redirect lives at a different path from
# versioned releases, so we handle them separately and always prefix tags with 'v'.
if [ "${BAKERY_VERSION}" = "latest" ]; then
    DL_PRIMARY="https://dl.breadway.dev/bakery/latest/bakery-x86_64"
    DL_FALLBACK="https://github.com/Breadway/bread-ecosystem/releases/latest/download/bakery-x86_64"
    SHA256_URL="https://dl.breadway.dev/bakery/latest/bakery-x86_64.sha256"
else
    # Strip a leading 'v' if the caller included it, then add it back consistently.
    ver="${BAKERY_VERSION#v}"
    DL_PRIMARY="https://dl.breadway.dev/bakery/${ver}/bakery-x86_64"
    DL_FALLBACK="https://github.com/Breadway/bread-ecosystem/releases/download/v${ver}/bakery-x86_64"
    SHA256_URL="https://dl.breadway.dev/bakery/${ver}/bakery-x86_64.sha256"
fi

# Pick a download tool.
if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q "$1" -O "$2"; }
else
    die "curl or wget required"
fi

mkdir -p "${BIN_DIR}"
TMP="$(mktemp)"
trap 'rm -f "${TMP}" "${TMP}.sha256"' EXIT

echo "downloading bakery…"
if fetch "${DL_PRIMARY}" "${TMP}" 2>/dev/null; then
    echo "  from dl.breadway.dev"
    # Verify checksum when available from primary.
    if fetch "${SHA256_URL}" "${TMP}.sha256" 2>/dev/null; then
        expected="$(awk '{print $1}' "${TMP}.sha256")"
        actual="$(sha256sum "${TMP}" | awk '{print $1}')"
        if [ "${expected}" != "${actual}" ]; then
            die "SHA-256 checksum mismatch (expected ${expected}, got ${actual})"
        fi
        echo "  checksum verified"
    else
        echo "  warning: could not fetch checksum — skipping verification"
    fi
elif fetch "${DL_FALLBACK}" "${TMP}" 2>/dev/null; then
    echo "  from GitHub (fallback)"
    # No .sha256 on the GitHub fallback path; proceed without verification.
    echo "  warning: checksum not verified for GitHub fallback download"
else
    die "failed to download bakery from both primary and fallback URLs"
fi

chmod +x "${TMP}"
cp "${TMP}" "${BIN_DIR}/bakery"
echo "installed bakery to ${BIN_DIR}/bakery"

# Warn if bin dir is not on PATH.
case ":${PATH}:" in
    *":${BIN_DIR}:"*) ;;
    *)
        echo ""
        echo "  note: ${BIN_DIR} is not in PATH — add to your shell profile:"
        echo "    export PATH=\"${BIN_DIR}:\$PATH\""
        ;;
esac

echo ""
echo "get started:"
echo "  bakery list                  # see all available packages"
echo "  bakery install bread         # install the automation daemon"
echo "  bakery install breadbar      # install the status bar"
echo "  bakery install breadpad      # install the scratchpad"
