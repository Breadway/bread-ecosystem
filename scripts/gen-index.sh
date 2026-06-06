#!/usr/bin/env bash
# Generate dl.breadway.dev/index.json from:
#   - registry/bread-ecosystem.toml   (product list)
#   - <repo>/bakery.toml              (per-product metadata)
#   - /srv/breadway-dl/               (built binaries + sha256 files)
#
# Run on hestia after each product build, before the dl server is refreshed.
# Requires: jq, python3 (for toml parsing via tomllib), sha256sum
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DL_DIR="${DL_DIR:-/srv/breadway-dl}"
DL_BASE="${DL_BASE:-https://dl.breadway.dev}"
GH_BASE="https://github.com/Breadway"
OUT="${DL_DIR}/index.json"

# Products are read from the registry. Each line is "name repo".
products=(
    "bakery          Breadway/bread-ecosystem"
    "bread           Breadway/bread"
    "breadbar        Breadway/breadbar"
    "breadbox        Breadway/breadbox"
    "breadcrumbs     Breadway/breadcrumbs"
    "breadpad        Breadway/breadpad"
)

# Build a JSON package entry for one product.
# $1 = product name, $2 = github repo slug
build_package_json() {
    local name="$1"
    local repo="$2"

    # Find the latest version dir under DL_DIR/<name>/
    local pkg_dir="${DL_DIR}/${name}"
    if [[ ! -d "${pkg_dir}" ]]; then
        echo "  warning: no release dir for ${name} at ${pkg_dir}" >&2
        return
    fi

    # The latest symlink must point to the current version dir.
    local latest_link="${pkg_dir}/latest"
    if [[ ! -L "${latest_link}" ]]; then
        echo "  warning: no 'latest' symlink for ${name}" >&2
        return
    fi
    local version_dir
    version_dir="$(readlink -f "${latest_link}")"
    local version
    version="$(basename "${version_dir}")"

    # Collect all binaries in the version dir (files without .sha256 extension).
    local binaries_json="[]"
    for bin_path in "${version_dir}"/*; do
        [[ "${bin_path}" == *.sha256 ]] && continue
        [[ -f "${bin_path}" ]] || continue
        local bin_name
        bin_name="$(basename "${bin_path}")"
        local sha256_path="${bin_path}.sha256"
        local sha256=""
        if [[ -f "${sha256_path}" ]]; then
            sha256="$(awk '{print $1}' "${sha256_path}")"
        fi
        local dl_url="${DL_BASE}/${name}/${version}/${bin_name}"
        local gh_url="${GH_BASE}/${repo}/releases/download/v${version}/${bin_name}"

        local entry
        entry="$(jq -n \
            --arg name "${bin_name}" \
            --arg dl_url "${dl_url}" \
            --arg github_url "${gh_url}" \
            --arg sha256 "${sha256}" \
            '{name: $name, dl_url: $dl_url, github_url: $github_url, sha256: $sha256}')"
        binaries_json="$(jq -n --argjson arr "${binaries_json}" --argjson e "${entry}" '$arr + [$e]')"
    done

    # Read bakery.toml: the release workflow copies it to DL_DIR alongside the
    # binaries; fall back to a sibling checkout for local dev use.
    local bakery_toml="${DL_DIR}/${name}/bakery.toml"
    if [[ ! -f "${bakery_toml}" ]]; then
        bakery_toml="${SCRIPT_DIR}/../${name}/bakery.toml"
    fi
    local description=""
    local system_deps="[]"
    local bread_deps="[]"
    local services="[]"
    local config="null"
    local post_install="[]"

    if [[ -f "${bakery_toml}" ]]; then
        description="$(python3 -c "
import tomllib, sys
with open('${bakery_toml}', 'rb') as f:
    d = tomllib.load(f)
print(d.get('description', ''))
" 2>/dev/null || true)"
        system_deps="$(python3 -c "
import tomllib, json, sys
with open('${bakery_toml}', 'rb') as f:
    d = tomllib.load(f)
print(json.dumps(d.get('system_deps', [])))
" 2>/dev/null || echo "[]")"
        bread_deps="$(python3 -c "
import tomllib, json, sys
with open('${bakery_toml}', 'rb') as f:
    d = tomllib.load(f)
print(json.dumps(d.get('bread_deps', [])))
" 2>/dev/null || echo "[]")"
        post_install="$(python3 -c "
import tomllib, json, sys
with open('${bakery_toml}', 'rb') as f:
    d = tomllib.load(f)
print(json.dumps(d.get('install', {}).get('post_install', [])))
" 2>/dev/null || echo "[]")"
    fi

    jq -n \
        --arg name "${name}" \
        --arg description "${description}" \
        --arg version "${version}" \
        --argjson binaries "${binaries_json}" \
        --argjson system_deps "${system_deps}" \
        --argjson bread_deps "${bread_deps}" \
        --argjson services "${services}" \
        --argjson post_install "${post_install}" \
        '{
            name: $name,
            description: $description,
            version: $version,
            binaries: $binaries,
            system_deps: $system_deps,
            bread_deps: $bread_deps,
            services: $services,
            post_install: $post_install
        }'
}

# Assemble the full index.
packages_json="{}"
for entry in "${products[@]}"; do
    name="$(echo "${entry}" | awk '{print $1}')"
    repo="$(echo "${entry}" | awk '{print $2}')"
    echo "processing ${name}…"
    pkg="$(build_package_json "${name}" "${repo}" 2>&1)" || { echo "  skipping ${name}: ${pkg}"; continue; }
    [[ -z "${pkg}" ]] && continue
    packages_json="$(jq -n --argjson m "${packages_json}" --arg k "${name}" --argjson v "${pkg}" '$m + {($k): $v}')"
done

jq -n \
    --arg version "1" \
    --arg generated_at "$(date -u +"%Y-%m-%dT%H:%M:%SZ")" \
    --argjson packages "${packages_json}" \
    '{version: $version, generated_at: $generated_at, packages: $packages}' \
    > "${OUT}"

echo "wrote ${OUT}"
