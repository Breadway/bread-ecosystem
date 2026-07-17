#!/usr/bin/env bash
# cleanup-old-mirror-workflows.sh — retire the per-repo GitHub mirroring
# pattern now that Forgejo native Push Mirrors (see setup-push-mirrors.sh)
# do the same job centrally.
#
# THIS SCRIPT IS DESTRUCTIVE AND TOUCHES LIVE, RUNNING INFRASTRUCTURE:
#   1. Deletes .forgejo/workflows/mirror.yml from the DEFAULT BRANCH of every
#      repo returned by the Forgejo API that has one (via the contents API —
#      this is a real commit to each repo's default branch, not a local/
#      worktree change).
#   2. Deletes the MIRROR_TOKEN Actions secret from every repo that has one.
#
# Do not run this until you have confirmed, for real, that push mirrors
# created by setup-push-mirrors.sh are actually syncing to GitHub (check
# a repo's Settings > Push Mirrors in the Forgejo web UI, or GET
# /repos/{owner}/{repo}/push_mirrors and look at last_update / last_error,
# and confirm commits are actually landing on the GitHub side). Until then,
# removing mirror.yml would silently kill the only thing currently keeping
# GitHub in sync.
#
# As a guardrail, this script refuses to do anything unless invoked with
# --i-have-verified-push-mirrors-work. There is no way around that flag
# short of editing this script, which is the point.
#
# Requires: bash, curl, jq
#
# Reads the same token file as setup-push-mirrors.sh:
#   FORGEJO_TOKEN_FILE   default ~/.config/forgejo/token
#
# Env vars:
#   FORGEJO_BASE     https://git.breadway.dev
#   FORGEJO_OWNER    Breadway
#
# Flags:
#   --i-have-verified-push-mirrors-work   required, see above
#   --dry-run                             print what would be deleted, make
#                                          no changes (combine with the
#                                          confirmation flag or this refuses
#                                          to run at all — even dry-run mode
#                                          is gated, so nobody can quietly
#                                          drop the guardrail out of the
#                                          invocation by force of habit)
#   --only repo1,repo2                    comma-separated allowlist
#
# Usage (once verified):
#   scripts/cleanup-old-mirror-workflows.sh --i-have-verified-push-mirrors-work --dry-run
#   scripts/cleanup-old-mirror-workflows.sh --i-have-verified-push-mirrors-work

set -euo pipefail

FORGEJO_BASE="${FORGEJO_BASE:-https://git.breadway.dev}"
FORGEJO_OWNER="${FORGEJO_OWNER:-Breadway}"
FORGEJO_TOKEN_FILE="${FORGEJO_TOKEN_FILE:-${HOME}/.config/forgejo/token}"

CONFIRMED=0
DRY_RUN=0
ONLY_REPOS=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --i-have-verified-push-mirrors-work) CONFIRMED=1; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        --only) ONLY_REPOS="$2"; shift 2 ;;
        -h|--help)
            sed -n '2,42p' "$0"
            exit 0
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

if [[ "${CONFIRMED}" != 1 ]]; then
    cat >&2 <<'EOF'
error: refusing to run.

This script deletes mirror.yml from the default branch of every mirrored
repo and removes the MIRROR_TOKEN secret. That's a real, immediate change
to production CI on every one of those repos, and it also permanently
disables the *old* mirroring path.

Before running this:
  1. Run setup-push-mirrors.sh for real (not --dry-run).
  2. Confirm, for at least one repo, that the push mirror actually synced
     (Forgejo web UI: repo Settings > Push Mirrors > check "Last Update"
     and that there's no "Last Error"; and check the GitHub side directly).
  3. Only then re-run this script with:
       --i-have-verified-push-mirrors-work

Add --dry-run (in addition to the flag above) to preview without changing
anything.
EOF
    exit 1
fi

for bin in curl jq; do
    command -v "${bin}" >/dev/null 2>&1 || { echo "error: ${bin} is required" >&2; exit 2; }
done

[[ -f "${FORGEJO_TOKEN_FILE}" ]] || { echo "error: Forgejo token file not found at ${FORGEJO_TOKEN_FILE}" >&2; exit 2; }
FORGEJO_TOKEN="$(<"${FORGEJO_TOKEN_FILE}")"

api() {
    # api METHOD PATH [JSON_BODY] -> prints response body, exits nonzero on HTTP error
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

owner_kind="org"
if ! curl -fsS -o /dev/null -H "Authorization: token ${FORGEJO_TOKEN}" \
        "${FORGEJO_BASE}/api/v1/orgs/${FORGEJO_OWNER}" 2>/dev/null; then
    owner_kind="user"
fi

if [[ "${owner_kind}" == "org" ]]; then
    repos_json="$(api GET "/orgs/${FORGEJO_OWNER}/repos?limit=50")"
else
    repos_json="$(api GET "/users/${FORGEJO_OWNER}/repos?limit=50")"
fi

mapfile -t repo_names < <(echo "${repos_json}" | jq -r '.[].name')

if [[ "${DRY_RUN}" == 1 ]]; then
    echo "# --dry-run: no deletions will be made"
fi
echo

for name in "${repo_names[@]}"; do
    if [[ -n "${ONLY_REPOS}" ]]; then
        IFS=',' read -ra allow <<< "${ONLY_REPOS}"
        match=0
        for a in "${allow[@]}"; do [[ "${a}" == "${name}" ]] && match=1; done
        [[ "${match}" == 1 ]] || continue
    fi

    default_branch="$(echo "${repos_json}" | jq -r --arg n "${name}" '.[] | select(.name==$n) | .default_branch')"

    # Contents API: GET returns the file's sha, which the DELETE call needs.
    file_info="$(curl -fsS -o /tmp/cleanup_probe.json -w '%{http_code}' \
        -H "Authorization: token ${FORGEJO_TOKEN}" \
        "${FORGEJO_BASE}/api/v1/repos/${FORGEJO_OWNER}/${name}/contents/.forgejo/workflows/mirror.yml?ref=${default_branch}" || true)"

    if [[ "${file_info}" == "200" ]]; then
        sha="$(jq -r '.sha' /tmp/cleanup_probe.json)"
        if [[ "${DRY_RUN}" == 1 ]]; then
            echo "WOULD-DELETE ${name}: .forgejo/workflows/mirror.yml (sha ${sha}) from ${default_branch}"
        else
            echo "DELETE ${name}: .forgejo/workflows/mirror.yml from ${default_branch}"
            del_body="$(jq -n --arg msg "ci: remove mirror.yml, superseded by native push mirror" \
                --arg sha "${sha}" --arg branch "${default_branch}" \
                '{message: $msg, sha: $sha, branch: $branch}')"
            api DELETE "/repos/${FORGEJO_OWNER}/${name}/contents/.forgejo/workflows/mirror.yml" "${del_body}" >/dev/null
        fi
    else
        echo "SKIP   ${name}: no .forgejo/workflows/mirror.yml on ${default_branch}"
    fi

    secret_check="$(curl -fsS -o /dev/null -w '%{http_code}' \
        -H "Authorization: token ${FORGEJO_TOKEN}" \
        "${FORGEJO_BASE}/api/v1/repos/${FORGEJO_OWNER}/${name}/actions/secrets" || true)"
    has_mirror_token="$(curl -fsS -H "Authorization: token ${FORGEJO_TOKEN}" \
        "${FORGEJO_BASE}/api/v1/repos/${FORGEJO_OWNER}/${name}/actions/secrets" \
        | jq -r '[.[] | select(.name=="MIRROR_TOKEN")] | length')"

    if [[ "${has_mirror_token}" -gt 0 ]]; then
        if [[ "${DRY_RUN}" == 1 ]]; then
            echo "WOULD-DELETE ${name}: MIRROR_TOKEN secret"
        else
            echo "DELETE ${name}: MIRROR_TOKEN secret"
            curl -fsS -X DELETE -H "Authorization: token ${FORGEJO_TOKEN}" \
                "${FORGEJO_BASE}/api/v1/repos/${FORGEJO_OWNER}/${name}/actions/secrets/MIRROR_TOKEN" >/dev/null
        fi
    else
        echo "SKIP   ${name}: no MIRROR_TOKEN secret"
    fi
done

rm -f /tmp/cleanup_probe.json
