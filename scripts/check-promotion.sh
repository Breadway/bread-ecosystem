#!/usr/bin/env bash
# check-promotion.sh - refuse a stable bakery release that did not go through
# the beta track.
#
# Policy (docs/release-channels.md): a stable tag `vX.Y.Z` is only allowed
# when a release candidate tag `vX.Y.Z-rc.N` points at the exact same commit
# AND that RC was published to the beta track at least MIN_AGE_DAYS earlier.
# This makes beta a real stage: the source state that ships as stable is the
# source state somebody actually ran from dl.breadway.dev/beta for a while.
#
# Usage:
#   scripts/check-promotion.sh REPO PKG TAG COMMIT [options]
#
#   REPO    Forgejo slug, e.g. Breadway/breadbar
#   PKG     bakery package name, e.g. breadbar
#   TAG     stable tag being released, e.g. v0.4.0
#   COMMIT  full commit id the tag points at (the workflow's checked-out HEAD)
#
# Options:
#   --min-age-days N   beta soak required before stable (default 3)
#   --beta-root DIR    beta artifact root (default /srv/breadway-dl/beta)
#   --remote URL       git remote used to list tags
#                      (default https://git.breadway.dev/$REPO.git)
#
# Environment (test hooks):
#   TAGS_FILE   read `git ls-remote --tags --refs`-style output from this file
#               instead of querying the remote
#   NOW_EPOCH   treat this Unix time as "now"
#
# Exit 0 when the tag may ship stable, 1 when the gate rejects it, 2 on a
# usage or environment error. Failures print `::error::` lines so Forgejo
# annotates the run.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/check-promotion.sh REPO PKG TAG COMMIT [options]

  REPO    Forgejo slug, e.g. Breadway/breadbar
  PKG     bakery package name, e.g. breadbar
  TAG     stable tag being released, e.g. v0.4.0
  COMMIT  full commit id the tag points at (the workflow's checked-out HEAD)

Options:
  --min-age-days N   beta soak required before stable (default 3)
  --beta-root DIR    beta artifact root (default /srv/breadway-dl/beta)
  --remote URL       git remote used to list tags
                     (default https://git.breadway.dev/REPO.git)

Exit 0 when the tag may ship stable, 1 when the gate rejects it, 2 on a
usage or environment error. See docs/release-channels.md.
EOF
    exit 2
}

die_usage() {
    echo "::error::check-promotion: $*" >&2
    exit 2
}

fail() {
    local msg
    for msg in "$@"; do
        echo "::error::check-promotion: ${msg}" >&2
    done
    exit 1
}

[[ $# -ge 4 ]] || usage

REPO="$1"
PKG="$2"
TAG="$3"
COMMIT="$4"
shift 4

MIN_AGE_DAYS=3
BETA_ROOT="${BETA_ROOT:-/srv/breadway-dl/beta}"
REMOTE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --min-age-days)
            [[ $# -ge 2 ]] || die_usage "--min-age-days needs a value"
            MIN_AGE_DAYS="$2"
            shift 2
            ;;
        --beta-root)
            [[ $# -ge 2 ]] || die_usage "--beta-root needs a value"
            BETA_ROOT="$2"
            shift 2
            ;;
        --remote)
            [[ $# -ge 2 ]] || die_usage "--remote needs a value"
            REMOTE="$2"
            shift 2
            ;;
        -h|--help)
            usage
            ;;
        *)
            die_usage "unknown argument: $1"
            ;;
    esac
done

[[ -n "${REPO}" ]] || die_usage "REPO is empty"
[[ -n "${PKG}" ]] || die_usage "PKG is empty"
[[ "${MIN_AGE_DAYS}" =~ ^[0-9]+$ ]] || die_usage "--min-age-days must be an integer"
[[ "${COMMIT}" =~ ^[0-9a-f]{40}$ ]] || die_usage "COMMIT must be a full 40-character commit id, got '${COMMIT}'"

VERSION="${TAG#v}"
if [[ "${VERSION}" == "${TAG}" ]]; then
    die_usage "TAG must start with 'v', got '${TAG}'"
fi
if [[ ! "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    fail "TAG '${TAG}' is not a stable release tag: stable tags are vX.Y.Z, prereleases belong on the beta track and never reach this gate"
fi

REMOTE="${REMOTE:-https://git.breadway.dev/${REPO}.git}"
now="${NOW_EPOCH:-$(date +%s)}"
min_age_secs=$((MIN_AGE_DAYS * 86400))

# Recorded force-promotion overrides (docs/release-channels.md, "Promotion
# overrides"). A gate rejection may be overridden only by a checked-in record
# naming this exact package, version and commit, plus a non-empty reason and
# an accountable author and date. The record lives in git history and every
# overridden release prints ::warning:: lines, so the exception is visible in
# the run log, not silent.
OVERRIDE_DIR="${PROMOTION_OVERRIDE_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/promotion-overrides}"

override_field() {
    # override_field <file> <key>: print the quoted value of `key = "value"`.
    sed -n -E "s/^[[:space:]]*$2[[:space:]]*=[[:space:]]*\"([^\"]*)\".*/\1/p" "$1" | head -1
}

recorded_override_for() {
    local file="${OVERRIDE_DIR}/${PKG}-${VERSION}.override"
    [[ -f "${file}" ]] || return 1

    local o_pkg o_version o_commit o_reason o_by o_at
    o_pkg="$(override_field "${file}" package)"
    o_version="$(override_field "${file}" version)"
    o_commit="$(override_field "${file}" commit)"
    o_reason="$(override_field "${file}" reason)"
    o_by="$(override_field "${file}" recorded_by)"
    o_at="$(override_field "${file}" recorded_at)"

    if [[ "${o_pkg}" != "${PKG}" || "${o_version}" != "${VERSION}" || "${o_commit}" != "${COMMIT}" ]]; then
        echo "::error::check-promotion: ${file} does not match this release (package=${o_pkg}, version=${o_version}, commit=${o_commit}); refusing the override." >&2
        return 2
    fi
    if [[ -z "${o_reason}" || -z "${o_by}" || -z "${o_at}" ]]; then
        echo "::error::check-promotion: ${file} is missing reason, recorded_by or recorded_at; refusing the override." >&2
        return 2
    fi

    echo "::warning::check-promotion: PROMOTION GATE OVERRIDDEN for ${PKG} ${VERSION} at ${COMMIT}"
    echo "::warning::check-promotion: reason: ${o_reason}"
    echo "::warning::check-promotion: recorded by ${o_by} on ${o_at} (${file})"
    return 0
}

fail_gate() {
    local rc=0
    recorded_override_for || rc=$?
    if (( rc == 0 )); then
        printf 'promotion gate: override record accepted; stable release continues without a soaked beta RC\n'
        exit 0
    fi
    # rc == 2 means an override file exists but is malformed or for another
    # commit: report that as a hard error instead of falling back to the
    # ordinary rejection message, so the broken record cannot hide.
    if (( rc == 2 )); then
        exit 1
    fi
    fail "$@"
}

# Collect every v<VERSION>-rc.N tag and the commit it ultimately points at.
# Annotated tags appear twice in ls-remote (the tag object and `<ref>^{}`,
# the commit it peels to); prefer the peeled line whichever order it lands.
if [[ -n "${TAGS_FILE:-}" ]]; then
    [[ -r "${TAGS_FILE}" ]] || die_usage "TAGS_FILE '${TAGS_FILE}' is not readable"
    raw_tags="$(cat "${TAGS_FILE}")"
else
    if ! raw_tags="$(git ls-remote --tags "${REMOTE}" "refs/tags/v${VERSION}-rc.*")"; then
        die_usage "cannot list tags from ${REMOTE}"
    fi
fi

declare -A tag_sha=()
while read -r sha ref; do
    [[ -z "${sha}" || -z "${ref}" ]] && continue
    ref="${ref#refs/tags/}"
    if [[ "${ref}" == *'^{}' ]]; then
        ref="${ref%'^{}'}"
        tag_sha["${ref}"]="${sha}"
    elif [[ -z "${tag_sha[${ref}]:-}" ]]; then
        tag_sha["${ref}"]="${sha}"
    fi
done <<< "${raw_tags}"

version_re="${VERSION//./\\.}"
mapfile -t all_rc_tags < <(
    printf '%s\n' "${!tag_sha[@]}" \
        | grep -E "^v${version_re}-rc\.[0-9]+$" \
        | sort -V || true
)

if [[ ${#all_rc_tags[@]} -eq 0 ]]; then
    fail_gate \
        "stable tag ${TAG} has no release candidate: no v${VERSION}-rc.N tag exists." \
        "Cut an RC from the exact commit being promoted, publish it to beta, and let it soak for ${MIN_AGE_DAYS} days:" \
        "  git tag v${VERSION}-rc.1 ${COMMIT} && git push origin v${VERSION}-rc.1"
fi

matching_rc_tags=()
for rc_tag in "${all_rc_tags[@]}"; do
    if [[ "${tag_sha[${rc_tag}]}" == "${COMMIT}" ]]; then
        matching_rc_tags+=("${rc_tag}")
    fi
done

if [[ ${#matching_rc_tags[@]} -eq 0 ]]; then
    detail=""
    for rc_tag in "${all_rc_tags[@]}"; do
        detail+=" ${rc_tag}->${tag_sha[${rc_tag}]}"
    done
    fail_gate \
        "stable tag ${TAG} points at ${COMMIT}, but no RC points at that commit." \
        "RC tags for ${VERSION} point elsewhere:${detail}" \
        "Fix the code first, then cut an RC from that fix: stable must reuse the exact commit that was tested on beta."
fi

best_rc=""
best_age=-1
best_dir=""
not_published=()
too_young=()

for rc_tag in "${matching_rc_tags[@]}"; do
    rc_version="${rc_tag#v}"
    rc_dir="${BETA_ROOT}/${PKG}/${rc_version}"

    if [[ ! -d "${rc_dir}" ]]; then
        not_published+=("${rc_tag}")
        continue
    fi
    if [[ ! -f "${rc_dir}/bakery.toml" ]]; then
        not_published+=("${rc_tag}")
        continue
    fi
    if ! compgen -G "${rc_dir}/*.sha256" >/dev/null 2>&1; then
        not_published+=("${rc_tag}")
        continue
    fi

    mtime="$(stat -c %Y "${rc_dir}" 2>/dev/null || stat -f %m "${rc_dir}" 2>/dev/null || echo 0)"
    [[ "${mtime}" =~ ^[0-9]+$ ]] || mtime=0
    age=$((now - mtime))
    if (( age < 0 )); then
        age=0
    fi

    if (( age >= min_age_secs )); then
        if (( age > best_age )); then
            best_rc="${rc_tag}"
            best_age="${age}"
            best_dir="${rc_dir}"
        fi
    else
        too_young+=("${rc_tag} (published $(date -u -d "@${mtime}" +%Y-%m-%dT%H:%MZ), ready $(date -u -d "@$((mtime + min_age_secs))" +%Y-%m-%dT%H:%MZ))")
    fi
done

if [[ -n "${best_rc}" ]]; then
    printf 'promotion gate: %s reuses RC %s on %s; beta build in %s is %s day(s) old (minimum %s)\n' \
        "${TAG}" "${best_rc}" "${COMMIT}" "${best_dir}" "$((best_age / 86400))" "${MIN_AGE_DAYS}"
    exit 0
fi

if [[ ${#too_young[@]} -gt 0 ]]; then
    fail_gate \
        "stable tag ${TAG} reuses a commit that was tested on beta, but not for long enough." \
        "Candidate(s) still soaking: ${too_young[*]}" \
        "The minimum beta soak is ${MIN_AGE_DAYS} days. Wait for the ready time above, or cut rc.N+1 once the next fix lands." \
        "If a hotfix cannot wait, that is a deliberate exception: do not bypass this gate in CI, ship the RC on beta and move the stable date."
fi

if [[ ${#not_published[@]} -gt 0 ]]; then
    fail_gate \
        "stable tag ${TAG} reuses RC commit ${COMMIT}, but the RC was never published to beta." \
        "Missing beta artifacts under ${BETA_ROOT}/${PKG}/ for: ${not_published[*]}" \
        "Check that the rc-release workflow for those tags went green and wrote its version directory, then retry the stable release."
fi

fail_gate "stable tag ${TAG} failed the beta promotion gate for an unclassified reason; inspect the RC tags and ${BETA_ROOT}/${PKG}/ manually."
