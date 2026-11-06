#!/usr/bin/env bash
# Regression tests for scripts/check-promotion.sh.
#
# Self-contained: no network, no writes outside a temp dir. Each case builds
# a fake beta artifact root and a `git ls-remote` output file, then asserts
# the gate's exit status.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="${HERE}/check-promotion.sh"

NOW=1800000000
V=1.2.3
C=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
D=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb

TMP="$(mktemp -d "${TMPDIR:-/tmp}/check-promotion-test.XXXXXX")"
trap 'rm -rf -- "${TMP}"' EXIT

BETA="${TMP}/beta"
mkdir -p "${BETA}"
TAGS="${TMP}/tags"

pass=0
fail=0

make_beta() {
    # make_beta <pkg> <version> <age-days> [--no-sha] [--no-toml]
    local pkg="$1" version="$2" age_days="$3"
    shift 3
    local dir="${BETA}/${pkg}/${version}"
    local with_sha=1 with_toml=1
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --no-sha) with_sha=0 ;;
            --no-toml) with_toml=0 ;;
            *) echo "make_beta: unknown flag $1" >&2; exit 2 ;;
        esac
        shift
    done
    mkdir -p "${dir}"
    (( with_toml )) && : > "${dir}/bakery.toml"
    (( with_sha )) && : > "${dir}/pkg-x86_64.sha256"
    touch -d "@$((NOW - age_days * 86400))" "${dir}"
}

write_tags() {
    # write_tags <tag>:<sha> ...
    : > "${TAGS}"
    local spec tag sha
    for spec in "$@"; do
        tag="${spec%%:*}"
        sha="${spec#*:}"
        printf '%s\trefs/tags/%s\n' "${sha}" "${tag}" >> "${TAGS}"
    done
}

check() {
    # check <name> <expected-exit> <args...>
    local name="$1" expected="$2"
    shift 2
    local out rc
    set +e
    out="$(BETA_ROOT="${BETA}" TAGS_FILE="${TAGS}" NOW_EPOCH="${NOW}" \
        bash "${SCRIPT}" "$@" 2>&1)"
    rc=$?
    set -e
    if [[ "${rc}" -eq "${expected}" ]]; then
        pass=$((pass + 1))
        printf 'ok   %s\n' "${name}"
    else
        fail=$((fail + 1))
        printf 'FAIL %s (expected exit %s, got %s)\n' "${name}" "${expected}" "${rc}" >&2
        printf '%s\n' "${out}" | sed 's/^/     /' >&2
    fi
}

# 1. RC on the exact commit, published to beta 4 days ago.
make_beta breadbar "${V}-rc.1" 4
write_tags "v${V}-rc.1:${C}"
check "passes when an RC on the same commit soaked past the minimum" 0 \
    Breadway/breadbar breadbar "v${V}" "${C}"

# 2. No RC tags at all.
write_tags
check "rejects a stable tag with no RC history" 1 \
    Breadway/breadbar breadbar "v${V}" "${C}"

# 3. RC exists, but on a different commit.
write_tags "v${V}-rc.1:${D}"
check "rejects an RC that points at a different commit" 1 \
    Breadway/breadbar breadbar "v${V}" "${C}"

# 4. RC on the right commit but still soaking.
make_beta breadbar "${V}-rc.2" 1
write_tags "v${V}-rc.2:${C}"
check "rejects an RC that has not soaked long enough" 1 \
    Breadway/breadbar breadbar "v${V}" "${C}"

# 5. RC on the right commit but never published to beta.
write_tags "v${V}-rc.3:${C}"
check "rejects an RC with no beta artifact directory" 1 \
    Breadway/breadbar breadbar "v${V}" "${C}"

# 6. Beta directory exists but has no checksummed binary.
make_beta breadbar "${V}-rc.4" 4 --no-sha
write_tags "v${V}-rc.4:${C}"
check "rejects a beta directory with no sha256 artifact" 1 \
    Breadway/breadbar breadbar "v${V}" "${C}"

# 7. Beta directory exists but no bakery.toml.
make_beta breadbar "${V}-rc.5" 4 --no-toml
write_tags "v${V}-rc.5:${C}"
check "rejects a beta directory with no bakery.toml" 1 \
    Breadway/breadbar breadbar "v${V}" "${C}"

# 8. Annotated tag: the peeled line supplies the commit.
make_beta breadbar "${V}-rc.6" 4
{
    printf '%s\trefs/tags/v%s-rc.6\n' "${D}" "${V}"
    printf '%s\trefs/tags/v%s-rc.6^{}\n' "${C}" "${V}"
} > "${TAGS}"
check "resolves an annotated RC tag to its commit" 0 \
    Breadway/breadbar breadbar "v${V}" "${C}"

# 9. An older RC on the same commit may carry the soak even when a newer RC
#    on that commit is fresh.
make_beta breadbar "${V}-rc.7" 5
make_beta breadbar "${V}-rc.8" 0
write_tags "v${V}-rc.7:${C}" "v${V}-rc.8:${C}"
check "passes when any RC on the same commit has finished soaking" 0 \
    Breadway/breadbar breadbar "v${V}" "${C}"

# 10. An RC for a different version does not count.
write_tags "v1.2.4-rc.1:${C}"
check "ignores RC tags for a different version" 1 \
    Breadway/breadbar breadbar "v${V}" "${C}"

# 11. Stable tags only: prerelease tags are rejected up front.
check "rejects a prerelease tag passed as a stable release" 1 \
    Breadway/breadbar breadbar "v${V}-rc.1" "${C}"

# 12. Commit ids must be full and hex.
check "rejects a short commit id" 2 \
    Breadway/breadbar breadbar "v${V}" "aaaaaaa"

# 13. A zero-day soak still requires the RC to exist and be published.
make_beta breadbar "${V}-rc.9" 0
write_tags "v${V}-rc.9:${C}"
check "accepts an explicitly zero-day soak when artifacts exist" 0 \
    Breadway/breadbar breadbar "v${V}" "${C}" --min-age-days 0

printf '\n%d passed, %d failed\n' "${pass}" "${fail}"
[[ "${fail}" -eq 0 ]]
