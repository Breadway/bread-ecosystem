#!/usr/bin/env bash
# setup-push-mirrors.sh — provision Forgejo native Push Mirrors to GitHub for
# every repo under the Breadway account, replacing the old per-repo
# .forgejo/workflows/mirror.yml + MIRROR_TOKEN pattern.
#
# THIS SCRIPT MUTATES LIVE FORGEJO STATE WHEN RUN WITHOUT --dry-run.
# Always run with --dry-run first and review the output before running for
# real. Nothing in this script deletes anything — see the separate
# cleanup-old-mirror-workflows.sh for removing the old mirror.yml files,
# which should only be run after confirming push mirrors are syncing.
#
# What it does, per repo returned by the Forgejo API:
#   1. GET  /repos/{owner}/{repo}/push_mirrors  — list existing push mirrors
#   2. If one already targets https://github.com/<gh-owner>/<repo>.git, skip
#      (idempotent — safe to re-run).
#   3. Otherwise POST /repos/{owner}/{repo}/push_mirrors to create one, with
#      sync_on_commit=true and a periodic interval as belt-and-suspenders.
#
# Requires: bash, curl, jq
#
# Reads (never prints the contents of either):
#   - A Forgejo API token from FORGEJO_TOKEN_FILE (default:
#     ~/.config/forgejo/token). Needs at least write access to repository
#     settings for every repo under the target account.
#   - A GitHub PAT from a dotenv-style `GH_TOKEN=...` line in
#     MIRROR_ENV_FILE (default: ~/.config/bread/mirror.env). The token needs
#     `repo` scope (classic PAT) or Contents: Read & Write (fine-grained) on
#     every target GitHub repo, since it's what actually pushes commits.
#
# Env vars (all optional, shown with defaults):
#   FORGEJO_BASE       https://git.breadway.dev
#   FORGEJO_OWNER      Breadway            # Forgejo account that owns the repos
#   GITHUB_OWNER       same as FORGEJO_OWNER   # GitHub account/org to mirror into
#   FORGEJO_TOKEN_FILE ~/.config/forgejo/token
#   MIRROR_ENV_FILE    ~/.config/bread/mirror.env
#   SYNC_INTERVAL      8h0m0s              # Forgejo duration string; periodic resync
#                                           # on top of sync_on_commit
#
# Flags:
#   --dry-run                 Print every GET/POST this script would make
#                              (including full request bodies except the
#                              GitHub token, which is redacted) without
#                              actually issuing any POST. GETs (listing repos,
#                              listing existing push mirrors) always happen —
#                              they're read-only and needed to print accurate
#                              dry-run output.
#   --include-private          By default, private Forgejo repos are SKIPPED
#                              and reported, not mirrored — pushing a private
#                              repo's history to a public GitHub repo is a
#                              one-way disclosure decision this script should
#                              never make silently. Pass this flag to include
#                              them anyway, after you've confirmed the target
#                              GitHub repo is also private (this script does
#                              not create or check GitHub-side repos or their
#                              visibility).
#   --only repo1,repo2         Comma-separated allowlist of repo names.
#                              Default: every repo the Forgejo API returns.
#
# Usage:
#   scripts/setup-push-mirrors.sh --dry-run
#   scripts/setup-push-mirrors.sh --dry-run --include-private
#   scripts/setup-push-mirrors.sh                     # the real thing

set -euo pipefail

FORGEJO_BASE="${FORGEJO_BASE:-https://git.breadway.dev}"
FORGEJO_OWNER="${FORGEJO_OWNER:-Breadway}"
GITHUB_OWNER="${GITHUB_OWNER:-${FORGEJO_OWNER}}"
FORGEJO_TOKEN_FILE="${FORGEJO_TOKEN_FILE:-${HOME}/.config/forgejo/token}"
MIRROR_ENV_FILE="${MIRROR_ENV_FILE:-${HOME}/.config/bread/mirror.env}"
SYNC_INTERVAL="${SYNC_INTERVAL:-8h0m0s}"

DRY_RUN=0
INCLUDE_PRIVATE=0
ONLY_REPOS=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) DRY_RUN=1; shift ;;
        --include-private) INCLUDE_PRIVATE=1; shift ;;
        --only) ONLY_REPOS="$2"; shift 2 ;;
        -h|--help)
            sed -n '2,55p' "$0"
            exit 0
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

for bin in curl jq; do
    command -v "${bin}" >/dev/null 2>&1 || { echo "error: ${bin} is required" >&2; exit 2; }
done

[[ -f "${FORGEJO_TOKEN_FILE}" ]] || { echo "error: Forgejo token file not found at ${FORGEJO_TOKEN_FILE}" >&2; exit 2; }
[[ -f "${MIRROR_ENV_FILE}" ]]   || { echo "error: mirror env file not found at ${MIRROR_ENV_FILE}" >&2; exit 2; }

FORGEJO_TOKEN="$(<"${FORGEJO_TOKEN_FILE}")"
GH_TOKEN="$(grep -m1 '^GH_TOKEN=' "${MIRROR_ENV_FILE}" | cut -d= -f2-)"
[[ -n "${GH_TOKEN}" ]] || { echo "error: no GH_TOKEN= line found in ${MIRROR_ENV_FILE}" >&2; exit 2; }

api() {
    # api METHOD PATH [JSON_BODY]
    local method="$1" path="$2" body="${3:-}"
    if [[ -n "${body}" ]]; then
        curl -fsS -X "${method}" \
            -H "Authorization: token ${FORGEJO_TOKEN}" \
            -H "Content-Type: application/json" \
            -d "${body}" \
            "${FORGEJO_BASE}/api/v1${path}"
    else
        curl -fsS -X "${method}" \
            -H "Authorization: token ${FORGEJO_TOKEN}" \
            "${FORGEJO_BASE}/api/v1${path}"
    fi
}

# Determine whether FORGEJO_OWNER is an org or a user — orgs and users use
# different list-repos endpoints.
owner_kind="org"
if ! curl -fsS -o /dev/null -H "Authorization: token ${FORGEJO_TOKEN}" \
        "${FORGEJO_BASE}/api/v1/orgs/${FORGEJO_OWNER}" 2>/dev/null; then
    owner_kind="user"
fi
echo "# ${FORGEJO_OWNER} is a Forgejo ${owner_kind} account"

if [[ "${owner_kind}" == "org" ]]; then
    repos_json="$(api GET "/orgs/${FORGEJO_OWNER}/repos?limit=50")"
else
    repos_json="$(api GET "/users/${FORGEJO_OWNER}/repos?limit=50")"
fi

mapfile -t repo_names < <(echo "${repos_json}" | jq -r '.[].name')
echo "# ${#repo_names[@]} repos found under ${FORGEJO_OWNER}"
echo

if [[ "${DRY_RUN}" == 1 ]]; then
    echo "# --dry-run: no POST requests will be made. GETs below are real, live reads."
    echo
fi

skipped_private=()
would_create=()
already_present=()

for name in "${repo_names[@]}"; do
    if [[ -n "${ONLY_REPOS}" ]]; then
        IFS=',' read -ra allow <<< "${ONLY_REPOS}"
        match=0
        for a in "${allow[@]}"; do [[ "${a}" == "${name}" ]] && match=1; done
        [[ "${match}" == 1 ]] || continue
    fi

    is_private="$(echo "${repos_json}" | jq -r --arg n "${name}" '.[] | select(.name==$n) | .private')"
    if [[ "${is_private}" == "true" && "${INCLUDE_PRIVATE}" == 0 ]]; then
        skipped_private+=("${name}")
        echo "SKIP  ${name}: private repo, pass --include-private to mirror it anyway"
        continue
    fi

    target_url="https://github.com/${GITHUB_OWNER}/${name}.git"

    existing="$(api GET "/repos/${FORGEJO_OWNER}/${name}/push_mirrors")"
    already="$(echo "${existing}" | jq -r --arg u "${target_url}" '[.[] | select(.remote_address==$u)] | length')"

    if [[ "${already}" -gt 0 ]]; then
        already_present+=("${name}")
        echo "OK    ${name}: push mirror to ${target_url} already exists, skipping"
        continue
    fi

    would_create+=("${name}")
    body="$(jq -n \
        --arg addr "${target_url}" \
        --arg user "x-access-token" \
        --arg pass "${GH_TOKEN}" \
        --arg interval "${SYNC_INTERVAL}" \
        '{remote_address: $addr, remote_username: $user, remote_password: $pass,
          sync_on_commit: true, interval: $interval, use_ssh: false}')"

    if [[ "${DRY_RUN}" == 1 ]]; then
        redacted="$(echo "${body}" | jq '.remote_password = "***REDACTED***"')"
        echo "WOULD-POST ${name}: /repos/${FORGEJO_OWNER}/${name}/push_mirrors"
        echo "${redacted}" | sed 's/^/    /'
    else
        echo "CREATE ${name}: push mirror -> ${target_url}"
        api POST "/repos/${FORGEJO_OWNER}/${name}/push_mirrors" "${body}" >/dev/null
    fi
done

echo
echo "# summary"
echo "#   already had a matching push mirror: ${#already_present[@]}"
echo "#   private, skipped (--include-private to override): ${#skipped_private[@]}"
if [[ "${DRY_RUN}" == 1 ]]; then
    echo "#   would create: ${#would_create[@]}"
else
    echo "#   created: ${#would_create[@]}"
fi
