#!/bin/sh
# Bootstrap script: downloads and installs the `bakery` binary.
# Usage: curl https://breadway.dev/get | sh
# Or:    curl -sSfL https://breadway.dev/get | sh
set -eu

# Pinned minisign public key for the bakery release binary. Matches the
# PUBKEY constant in bakery/src/manifest.rs (same keypair signs both
# index.json and the bakery binary itself). Do not source this from the
# network — it must be baked into this script so a compromised dl server
# can't swap it out along with a malicious binary.
BAKERY_MINISIGN_PUBKEY="RWTBR8w/IJ+jaylOv80b52DzekKbSR2CvOVGvzB0ipGBaMhJPAOiEWq8"

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
    SIG_URL="https://dl.breadway.dev/bakery/latest/bakery-x86_64.minisig"
    SIG_FALLBACK="https://github.com/Breadway/bread-ecosystem/releases/latest/download/bakery-x86_64.minisig"
else
    # Strip a leading 'v' if the caller included it, then add it back consistently.
    ver="${BAKERY_VERSION#v}"
    DL_PRIMARY="https://dl.breadway.dev/bakery/${ver}/bakery-x86_64"
    DL_FALLBACK="https://github.com/Breadway/bread-ecosystem/releases/download/v${ver}/bakery-x86_64"
    SHA256_URL="https://dl.breadway.dev/bakery/${ver}/bakery-x86_64.sha256"
    SIG_URL="https://dl.breadway.dev/bakery/${ver}/bakery-x86_64.minisig"
    SIG_FALLBACK="https://github.com/Breadway/bread-ecosystem/releases/download/v${ver}/bakery-x86_64.minisig"
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
trap 'rm -f "${TMP}" "${TMP}.sha256" "${TMP}.minisig"' EXIT

echo "downloading bakery…"
if fetch "${DL_PRIMARY}" "${TMP}" 2>/dev/null; then
    echo "  from dl.breadway.dev"
    sig_url="${SIG_URL}"
    checksum_only_fallback_note="  warning: could not fetch checksum — skipping verification"
elif fetch "${DL_FALLBACK}" "${TMP}" 2>/dev/null; then
    echo "  from GitHub (fallback)"
    sig_url="${SIG_FALLBACK}"
    checksum_only_fallback_note="  warning: no checksum available for GitHub fallback download"
else
    die "failed to download bakery from both primary and fallback URLs"
fi

# Signature verification is the authoritative check: it proves the binary
# was produced by whoever holds the bakery signing key, not just that bytes
# match whatever the same (possibly compromised) server also reports as the
# checksum. Prefer it whenever both a .minisig is published and a minisign
# verifier is available on this machine.
sig_verified=0
if fetch "${sig_url}" "${TMP}.minisig" 2>/dev/null; then
    if command -v minisign >/dev/null 2>&1; then
        if minisign -V -q -m "${TMP}" -x "${TMP}.minisig" -P "${BAKERY_MINISIGN_PUBKEY}"; then
            echo "  signature verified (minisign)"
            sig_verified=1
        else
            die "minisign signature verification FAILED — refusing to install a binary that doesn't match the pinned bakery key"
        fi
    else
        echo "  warning: 'minisign' is not installed — cannot verify the binary's" >&2
        echo "  warning: signature, only its checksum. Install minisign for the" >&2
        echo "  warning: strongest guarantee: pacman -S minisign / apt install minisign" >&2
    fi
else
    echo "  warning: no .minisig published for this release yet — signature not verified" >&2
fi

# Checksum is a secondary, best-effort check (kept for defense in depth and
# for the case where minisign isn't installed). It is not a substitute for
# signature verification: both the binary and its checksum typically come
# from the same server, so a compromised server can serve a matching pair.
if fetch "${SHA256_URL}" "${TMP}.sha256" 2>/dev/null; then
    expected="$(awk '{print $1}' "${TMP}.sha256")"
    actual="$(sha256sum "${TMP}" | awk '{print $1}')"
    if [ "${expected}" != "${actual}" ]; then
        die "SHA-256 checksum mismatch (expected ${expected}, got ${actual})"
    fi
    echo "  checksum verified"
else
    echo "${checksum_only_fallback_note}"
fi

if [ "${sig_verified}" -ne 1 ]; then
    echo "  warning: proceeding WITHOUT a verified signature on the bakery binary" >&2
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
