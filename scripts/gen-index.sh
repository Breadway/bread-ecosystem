#!/usr/bin/env bash
# Generate dl.breadway.dev/index.json from:
#   - registry/bread-ecosystem.toml   (product list)
#   - <DL_DIR>/<name>/bakery.toml     (per-product metadata, uploaded by release.yml)
#   - <DL_DIR>/                       (built binaries + sha256 files)
#
# Fallback for local dev: looks for ../name/bakery.toml (sibling repo checkout).
# Run on hestia after each product build, before the dl server is refreshed.
# Requires: jq, python3 (tomllib, stdlib since 3.11), sha256sum
set -euo pipefail

SCRIPT_DIR="${SCRIPT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
DL_DIR="${DL_DIR:-/srv/breadway-dl}"
DL_BASE="${DL_BASE:-https://dl.breadway.dev}"
GH_BASE="https://github.com"
OUT="${DL_DIR}/index.json"

# Read the product list from the registry TOML instead of a hardcoded array.
mapfile -t products < <(python3 -c "
import tomllib, sys
with open('${SCRIPT_DIR}/registry/bread-ecosystem.toml', 'rb') as f:
    d = tomllib.load(f)
for p in d['products']:
    print(p['name'], p['repo'])
")

# Build a JSON package entry for one product.
# $1 = product name, $2 = github repo slug
build_package_json() {
    local name="$1"
    local repo="$2"

    # Find the latest version dir under DL_DIR/<name>/
    local pkg_dir="${DL_DIR}/${name}"
    if [[ ! -d "${pkg_dir}" ]]; then
        echo "  warning: no release dir for ${name} at ${pkg_dir}" >&2
        return 1
    fi

    # The latest symlink must point to the current version dir.
    local latest_link="${pkg_dir}/latest"
    if [[ ! -L "${latest_link}" ]]; then
        echo "  warning: no 'latest' symlink for ${name}" >&2
        return 1
    fi
    local version_dir
    version_dir="$(readlink -f "${latest_link}")"
    local version
    version="$(basename "${version_dir}")"

    # Collect all binaries in the version dir (executables only; skip metadata files).
    local binaries_json="[]"
    for bin_path in "${version_dir}"/*; do
        [[ "${bin_path}" == *.sha256 ]]  && continue
        [[ "${bin_path}" == *.toml ]]    && continue
        [[ "${bin_path}" == *.service ]] && continue
        [[ "${bin_path}" == *.css ]]     && continue
        [[ "${bin_path}" == *.txt ]]     && continue
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

    # Locate bakery.toml: the release workflow copies it to DL_DIR alongside the
    # binaries.  Fall back to a sibling repo checkout for local dev use.
    local bakery_toml="${DL_DIR}/${name}/bakery.toml"
    if [[ ! -f "${bakery_toml}" ]]; then
        bakery_toml="${SCRIPT_DIR}/../${name}/bakery.toml"
    fi
    if [[ ! -f "${bakery_toml}" ]]; then
        echo "ERROR: bakery.toml not found for ${name} — release.yml must upload it to ${DL_DIR}/${name}/bakery.toml" >&2
        return 1
    fi

    local description system_deps optional_system_deps bread_deps services config post_install

    description="$(python3 -c "
import tomllib
with open('${bakery_toml}', 'rb') as f:
    d = tomllib.load(f)
print(d.get('description', ''))
" 2>/dev/null || true)"

    system_deps="$(python3 -c "
import tomllib, json
with open('${bakery_toml}', 'rb') as f:
    d = tomllib.load(f)
print(json.dumps(d.get('system_deps', [])))
" 2>/dev/null || echo "[]")"

    optional_system_deps="$(python3 -c "
import tomllib, json
with open('${bakery_toml}', 'rb') as f:
    d = tomllib.load(f)
print(json.dumps(d.get('optional_system_deps', [])))
" 2>/dev/null || echo "[]")"

    bread_deps="$(python3 -c "
import tomllib, json
with open('${bakery_toml}', 'rb') as f:
    d = tomllib.load(f)
print(json.dumps(d.get('bread_deps', [])))
" 2>/dev/null || echo "[]")"

    # [[service]] entries → [{unit, enable}]
    services="$(python3 -c "
import tomllib, json
with open('${bakery_toml}', 'rb') as f:
    d = tomllib.load(f)
svcs = d.get('service', [])
print(json.dumps([{'unit': s['unit'], 'enable': s.get('enable', False)} for s in svcs]))
" 2>/dev/null || echo "[]")"

    # [config] → {dir, example?} or null
    config="$(python3 -c "
import tomllib, json
with open('${bakery_toml}', 'rb') as f:
    d = tomllib.load(f)
cfg = d.get('config')
if cfg:
    obj = {'dir': cfg['dir']}
    if 'example' in cfg:
        obj['example'] = cfg['example']
    print(json.dumps(obj))
else:
    print('null')
" 2>/dev/null || echo "null")"

    post_install="$(python3 -c "
import tomllib, json
with open('${bakery_toml}', 'rb') as f:
    d = tomllib.load(f)
print(json.dumps(d.get('install', {}).get('post_install', [])))
" 2>/dev/null || echo "[]")"

    jq -n \
        --arg name "${name}" \
        --arg description "${description}" \
        --arg version "${version}" \
        --argjson binaries "${binaries_json}" \
        --argjson system_deps "${system_deps}" \
        --argjson optional_system_deps "${optional_system_deps}" \
        --argjson bread_deps "${bread_deps}" \
        --argjson services "${services}" \
        --argjson config "${config}" \
        --argjson post_install "${post_install}" \
        '{
            name: $name,
            description: $description,
            version: $version,
            binaries: $binaries,
            system_deps: $system_deps,
            optional_system_deps: $optional_system_deps,
            bread_deps: $bread_deps,
            services: $services,
            config: $config,
            post_install: $post_install
        }'
}

# Assemble the full index.
packages_json="{}"
for entry in "${products[@]}"; do
    name="$(echo "${entry}" | awk '{print $1}')"
    repo="$(echo "${entry}" | awk '{print $2}')"
    echo "processing ${name}…"
    pkg="$(build_package_json "${name}" "${repo}")" || { echo "  skipping ${name}"; continue; }
    [[ -z "${pkg}" ]] && { echo "  skipping ${name}: no output"; continue; }
    packages_json="$(jq -n --argjson m "${packages_json}" --arg k "${name}" --argjson v "${pkg}" '$m + {($k): $v}')"
done

jq -n \
    --arg version "1" \
    --arg generated_at "$(date -u +"%Y-%m-%dT%H:%M:%SZ")" \
    --argjson packages "${packages_json}" \
    '{version: $version, generated_at: $generated_at, packages: $packages}' \
    > "${OUT}"

echo "wrote ${OUT}"
