#!/usr/bin/env bash
# Smoke-test gen-index.sh against a minimal fixture DL_DIR tree.
# Verifies that services, config, system_deps, optional_system_deps,
# description, and post_install are all populated correctly, and that every
# systemd unit type is excluded from the binaries list (ECO-87).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE="$(mktemp -d)"
FAKE_REGISTRY="$(mktemp -d)"
trap 'rm -rf "${FIXTURE}" "${FAKE_REGISTRY}"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

# Every systemd unit type. A version dir may contain any of them, declared in
# bakery.toml or shipped alongside the binaries, and none of them is a binary.
UNIT_SUFFIXES=(service socket device mount automount swap target path timer slice scope)

# ── Build a minimal release tree for "fakepkg" ───────────────────────────────
PKG_VER_DIR="${FIXTURE}/fakepkg/0.1.0"
mkdir -p "${PKG_VER_DIR}"

printf 'fake-binary-content' > "${PKG_VER_DIR}/fakepkg-x86_64"
sha256sum "${PKG_VER_DIR}/fakepkg-x86_64" | awk '{print $1}' \
    > "${PKG_VER_DIR}/fakepkg-x86_64.sha256"
printf '# example config\n' > "${PKG_VER_DIR}/fakepkg.example.toml"

# For each unit type, ship one unit declared under [[service]] and one that is
# not declared at all. Neither may be published as a binary: an undeclared
# unit has no .sha256 sidecar, and a declared one already appears under
# services. bread really ships breadbug-status.timer this way (ECO-87).
for suffix in "${UNIT_SUFFIXES[@]}"; do
    printf '[Unit]\nDescription=fakepkg declared %s\n' "${suffix}" \
        > "${PKG_VER_DIR}/fakepkg.${suffix}"
    printf '[Unit]\nDescription=fakepkg undeclared %s\n' "${suffix}" \
        > "${PKG_VER_DIR}/fakepkg-status.${suffix}"
done

{
    cat <<'TOML'
name = "fakepkg"
description = "A fake package for testing"
binaries = ["fakepkg"]
system_deps = ["gtk4"]
optional_system_deps = ["hyprland"]
bread_deps = []
TOML
    for suffix in "${UNIT_SUFFIXES[@]}"; do
        printf '\n[[service]]\nunit = "fakepkg.%s"\nenable = true\n' "${suffix}"
    done
    cat <<'TOML'

[config]
dir = "~/.config/fakepkg"
example = "fakepkg.example.toml"

[install]
post_install = ["echo installed"]
TOML
} > "${PKG_VER_DIR}/bakery.toml"

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
GEN_LOG="${FIXTURE}/gen-index.log"
SCRIPT_DIR="${FAKE_REGISTRY}" DL_DIR="${FIXTURE}" DL_BASE="https://dl.test" \
    bash "${REPO_ROOT}/scripts/gen-index.sh" >"${GEN_LOG}" 2>&1 \
    || { sed 's/^/  [gen-index] /' "${GEN_LOG}"; fail "gen-index.sh exited non-zero"; }
sed 's/^/  [gen-index] /' "${GEN_LOG}"

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

check "post_install[0]" \
    "echo installed" \
    "$(jq -r '.packages.fakepkg.post_install[0]' "${OUT}")"

check "config.example_sha256" \
    "$(sha256sum "${PKG_VER_DIR}/fakepkg.example.toml" | awk '{print $1}')" \
    "$(jq -r '.packages.fakepkg.config.example_sha256' "${OUT}")"

# Every declared [[service]] entry, across the whole unit suffix set, must
# appear under services with the sha256 of the unit file that ships with it.
check "declared service count" \
    "${#UNIT_SUFFIXES[@]}" \
    "$(jq -r '.packages.fakepkg.services | length' "${OUT}")"

check "no declared service lacks a sha256" \
    "0" \
    "$(jq -r '[.packages.fakepkg.services[] | select((.sha256 // "") == "")] | length' "${OUT}")"

for suffix in "${UNIT_SUFFIXES[@]}"; do
    unit="fakepkg.${suffix}"
    check "services sha256 for ${unit}" \
        "$(sha256sum "${PKG_VER_DIR}/${unit}" | awk '{print $1}')" \
        "$(jq -r --arg unit "${unit}" \
            '.packages.fakepkg.services[] | select(.unit == $unit) | .sha256' "${OUT}")"
done

# No unit file may be published as a binary. A leaked unit has no .sha256
# sidecar, so it would carry an empty checksum, and bakery refuses to install
# a package whose artifact has no sha256 (ECO-87).
check "binaries are exactly the real binary" \
    "fakepkg-x86_64" \
    "$(jq -r '.packages.fakepkg.binaries | map(.name) | join(",")' "${OUT}")"

check "binary count" \
    "1" \
    "$(jq -r '.packages.fakepkg.binaries | length' "${OUT}")"

check "no binary lacks a sha256" \
    "0" \
    "$(jq -r '[.packages[].binaries[] | select((.sha256 // "") == "")] | length' "${OUT}")"

for suffix in "${UNIT_SUFFIXES[@]}"; do
    for unit in "fakepkg.${suffix}" "fakepkg-status.${suffix}"; do
        check "unit file is not a binary: ${unit}" \
            "null" \
            "$(jq -r --arg unit "${unit}" \
                '.packages.fakepkg.binaries | map(.name) | map(select(. == $unit)) | first' "${OUT}")"

        # The suffix rule must be what excludes a unit, not the generic
        # "no .sha256 sidecar" fallback: a missing suffix still stays out of
        # binaries thanks to that fallback, but logs a spurious warning for
        # every unit file, which is the noise this fix also removes. Asserting
        # on the warning keeps this fixture able to catch a regressed suffix
        # list on a tree that already has the sidecar guard.
        if grep -q "skipped '${unit}'" "${GEN_LOG}"; then
            fail "unit file ${unit} was not skipped by the unit-suffix rule (gen-index warned about it)"
        fi
    done
done

echo "OK: all gen-index assertions passed"
