#!/usr/bin/env bash
# doctor-channels.sh — detect drift between a repo's declared distribution
# channel(s) and its actual .forgejo/workflows/ + packaging metadata.
#
# See docs/release-channels.md for the policy this checks against.
#
# Usage:
#   scripts/doctor-channels.sh [BASE_DIR]
#
# BASE_DIR defaults to the parent of this repo checkout (i.e. run from a
# normal ~/Projects/bread-ecosystem checkout, it scans sibling ~/Projects/*
# repos). Point it at a directory of worktrees (e.g. ~/Projects, which is
# also where *-fix-worktree checkouts live) to check those instead:
#
#   scripts/doctor-channels.sh ~/Projects
#
# Exits 0 if no drift found, 1 if any repo has drift (so it's CI-friendly).
#
# Requires: python3 (tomllib, stdlib since 3.11)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE_DIR="${1:-$(dirname "${SCRIPT_DIR}")}"
REGISTRY="${SCRIPT_DIR}/registry/bread-ecosystem.toml"

if [[ ! -f "${REGISTRY}" ]]; then
    echo "error: registry not found at ${REGISTRY}" >&2
    exit 2
fi

# repo (last path segment of registry `repo = "Breadway/x"`) -> 1
mapfile -t registry_repos < <(python3 -c "
import tomllib
with open('${REGISTRY}', 'rb') as f:
    d = tomllib.load(f)
for p in d['products']:
    print(p['repo'].split('/')[-1])
")

is_in_registry() {
    local name="$1"
    for r in "${registry_repos[@]}"; do
        [[ "${r}" == "${name}" ]] && return 0
    done
    return 1
}

# Repos with a deliberately non-standard packaging shape that the
# single-PKGBUILD/single-package.yml heuristic below doesn't fit. Extend
# this if another repo grows a legitimately special-cased layout.
PACKAGE_CHECK_EXEMPT=("bos")   # ships an ISO via release-iso.yml; its PKGBUILDs
                                # under packaging/*/ build bundled AUR deps
                                # (bibata, calamares, ...), each with its own
                                # dedicated workflow — not a pacman-channel package.

is_package_check_exempt() {
    local name="$1"
    for r in "${PACKAGE_CHECK_EXEMPT[@]}"; do
        [[ "${r}" == "${name}" ]] && return 0
    done
    return 1
}

drift=0
checked=0

for dir in "${BASE_DIR}"/*/; do
    name="$(basename "${dir}")"
    name="${name%-fix-worktree}"   # normalize worktree checkouts back to the repo name
    [[ -d "${dir}/.git" || -f "${dir}/.git" ]] || continue
    # Skip bread-ecosystem itself — it's a multi-product repo the registry
    # membership check above doesn't map 1:1, and it's already reviewed by
    # hand above (bakery + bread-theme products).
    [[ "${name}" == "bread-ecosystem" ]] && continue

    checked=$((checked + 1))
    has_bakery_toml=0
    [[ -f "${dir}/bakery.toml" ]] && has_bakery_toml=1

    has_release_wf=0
    compgen -G "${dir}/.forgejo/workflows/release*.yml" >/dev/null 2>&1 && has_release_wf=1

    in_registry=0
    is_in_registry "${name}" && in_registry=1

    has_pkgbuild=0
    find "${dir}" -maxdepth 3 -iname 'PKGBUILD' -not -path '*/.git/*' 2>/dev/null \
        | grep -q . && has_pkgbuild=1

    has_package_wf=0
    [[ -f "${dir}/.forgejo/workflows/package.yml" ]] && has_package_wf=1

    issues=()

    if [[ "${has_bakery_toml}" == 1 && "${in_registry}" == 0 ]]; then
        issues+=("has bakery.toml but no registry/bread-ecosystem.toml entry")
    fi
    if [[ "${in_registry}" == 1 && "${has_bakery_toml}" == 0 ]]; then
        issues+=("registered in bread-ecosystem.toml but has no bakery.toml")
    fi
    if [[ "${in_registry}" == 1 && "${has_release_wf}" == 0 ]]; then
        issues+=("registered + has bakery.toml but no release*.yml workflow")
    fi
    if [[ "${has_bakery_toml}" == 1 && "${in_registry}" == 0 && "${has_release_wf}" == 1 ]]; then
        issues+=("has a release workflow for a product not in the registry (index.json will never include it)")
    fi
    if ! is_package_check_exempt "${name}"; then
        if [[ "${has_pkgbuild}" == 1 && "${has_package_wf}" == 0 ]]; then
            issues+=("has a PKGBUILD but no package.yml workflow")
        fi
        if [[ "${has_package_wf}" == 1 && "${has_pkgbuild}" == 0 ]]; then
            issues+=("has package.yml but no PKGBUILD")
        fi
    fi

    if [[ ${#issues[@]} -gt 0 ]]; then
        drift=1
        echo "${name}:"
        for i in "${issues[@]}"; do
            echo "  - ${i}"
        done
    fi
done

echo
echo "checked ${checked} repos under ${BASE_DIR}"
if [[ "${drift}" == 0 ]]; then
    echo "no channel drift found"
else
    echo "drift found — see docs/release-channels.md for the policy"
fi
exit "${drift}"
