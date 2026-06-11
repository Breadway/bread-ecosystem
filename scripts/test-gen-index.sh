#!/usr/bin/env bash
# Smoke-test gen-index.sh against a minimal fixture DL_DIR tree.
# Verifies that services, config, system_deps, optional_system_deps,
# description, and post_install are all populated correctly.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE="$(mktemp -d)"
FAKE_REGISTRY="$(mktemp -d)"
trap 'rm -rf "${FIXTURE}" "${FAKE_REGISTRY}"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

# ── Build a minimal release tree for "fakepkg" ───────────────────────────────
PKG_VER_DIR="${FIXTURE}/fakepkg/0.1.0"
mkdir -p "${PKG_VER_DIR}"

printf 'fake-binary-content' > "${PKG_VER_DIR}/fakepkg-x86_64"
sha256sum "${PKG_VER_DIR}/fakepkg-x86_64" | awk '{print $1}' \
    > "${PKG_VER_DIR}/fakepkg-x86_64.sha256"
printf '[Unit]\nDescription=fakepkg\n' > "${PKG_VER_DIR}/fakepkg.service"
printf '# example config\n' > "${PKG_VER_DIR}/fakepkg.example.toml"

cat > "${PKG_VER_DIR}/bakery.toml" <<'TOML'
name = "fakepkg"
description = "A fake package for testing"
binaries = ["fakepkg"]
system_deps = ["gtk4"]
optional_system_deps = ["hyprland"]
bread_deps = []

[[service]]
unit = "fakepkg.service"
enable = true

[config]
dir = "~/.config/fakepkg"
example = "fakepkg.example.toml"

[install]
post_install = ["echo installed"]
TOML

# gen-index looks for bakery.toml at ${DL_DIR}/<name>/bakery.toml (no version)
cp "${PKG_VER_DIR}/bakery.toml" "${FIXTURE}/fakepkg/bakery.toml"
ln -s "${PKG_VER_DIR}" "${FIXTURE}/fakepkg/latest"

# ── Minimal registry pointing only at fakepkg ────────────────────────────────
mkdir -p "${FAKE_REGISTRY}/registry"
cat > "${FAKE_REGISTRY}/registry/bread-ecosystem.toml" <<'TOML'
[ecosystem]
name = "test"

[[products]]
name = "fakepkg"
repo = "Test/fakepkg"
description = "A fake package"
TOML

# ── Run gen-index with overridden SCRIPT_DIR and DL_DIR ──────────────────────
OUT="${FIXTURE}/index.json"
SCRIPT_DIR="${FAKE_REGISTRY}" DL_DIR="${FIXTURE}" DL_BASE="https://dl.test" \
    bash "${REPO_ROOT}/scripts/gen-index.sh" 2>&1 | sed 's/^/  [gen-index] /'

[[ -f "${OUT}" ]] || fail "index.json was not produced"

# ── Assertions ────────────────────────────────────────────────────────────────
jq -e '.packages.fakepkg' "${OUT}" > /dev/null \
    || fail "fakepkg missing from index"

check() {
    local label="$1" expected="$2" actual="$3"
    [[ "${actual}" == "${expected}" ]] \
        || fail "${label}: expected '${expected}', got '${actual}'"
}

check "description" \
    "A fake package for testing" \
    "$(jq -r '.packages.fakepkg.description' "${OUT}")"

check "system_deps" \
    "gtk4" \
    "$(jq -r '.packages.fakepkg.system_deps | join(",")' "${OUT}")"

check "optional_system_deps" \
    "hyprland" \
    "$(jq -r '.packages.fakepkg.optional_system_deps | join(",")' "${OUT}")"

check "services[0].unit" \
    "fakepkg.service" \
    "$(jq -r '.packages.fakepkg.services[0].unit' "${OUT}")"

check "services[0].enable" \
    "true" \
    "$(jq -r '.packages.fakepkg.services[0].enable' "${OUT}")"

check "config.dir" \
    "~/.config/fakepkg" \
    "$(jq -r '.packages.fakepkg.config.dir' "${OUT}")"

check "config.example" \
    "fakepkg.example.toml" \
    "$(jq -r '.packages.fakepkg.config.example' "${OUT}")"

check "binaries[0].name" \
    "fakepkg-x86_64" \
    "$(jq -r '.packages.fakepkg.binaries[0].name' "${OUT}")"

check "post_install[0]" \
    "echo installed" \
    "$(jq -r '.packages.fakepkg.post_install[0]' "${OUT}")"

echo "OK: all gen-index assertions passed"
