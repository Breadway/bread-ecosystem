#!/bin/sh
# Bootstrap script: installs the `bakery` binary.
# Usage: curl https://breadway.dev/get | sh
# Or:    curl -sSfL https://breadway.dev/get | sh
set -eu

BAKERY_VERSION="${BAKERY_VERSION:-latest}"
DL_PRIMARY="https://dl.breadway.dev/bakery/${BAKERY_VERSION}/bakery-x86_64"
DL_FALLBACK="https://github.com/Breadway/bread-ecosystem/releases/download/${BAKERY_VERSION}/bakery-x86_64"
BIN_DIR="${BAKERY_BIN_DIR:-$HOME/.local/bin}"

die() { echo "error: $*" >&2; exit 1; }

# Verify platform.
uname -m | grep -q x86_64 || die "bakery only supports x86_64 (got $(uname -m))"
uname -s | grep -q Linux  || die "bakery only supports Linux (got $(uname -s))"

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
trap 'rm -f "${TMP}"' EXIT

echo "downloading bakery…"
if fetch "${DL_PRIMARY}" "${TMP}" 2>/dev/null; then
    echo "  from dl.breadway.dev"
elif fetch "${DL_FALLBACK}" "${TMP}" 2>/dev/null; then
    echo "  from GitHub (fallback)"
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
