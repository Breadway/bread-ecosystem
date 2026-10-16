#!/usr/bin/env bash
# ci-checkout.sh: check out the exact commit a CI event was created for, into
# a directory this job owns.
#
# Consumer check/release workflows used to run `rm -rf src && git clone
# --branch "$GITHUB_REF_NAME" src`. Jobs on the same self-hosted runner then
# share one relative path. The local control in test-ci-checkout.sh shows a
# second clone into that path replacing a tree another job is still reading
# (ECO-79). The runner's own workspace was not inspected, so this is the
# mechanism the logs and the control support, not a directly observed
# interleaving. This helper gives each job a unique directory and pins the
# tree to the event SHA instead of whatever the branch points to at run time.
#
# Usage:
#   scripts/ci-checkout.sh <repo-url> <sha> <dest>
#
# <repo-url> must carry no userinfo: no token, user, or password before the
# host. Use a plain https URL with the runner's credential helper, or the
# scp-style git@host:path form, which authenticates with an SSH key.
# <dest> must not exist yet; the caller creates it with mktemp -d so the
# directory is owned by this job.
#
# Exit status: 0 when HEAD of <dest> equals <sha>, non-zero otherwise. On any
# failure the partially created <dest> is removed here, so a failed attempt
# leaves nothing behind.
set -euo pipefail

if [ $# -ne 3 ]; then
    echo "usage: ci-checkout.sh <repo-url> <sha> <dest>" >&2
    exit 2
fi

REPO_URL="$1"
SHA="$2"
DEST="$3"

if [[ "${REPO_URL}" =~ ^[a-zA-Z+.-]+://[^/]*@ ]]; then
    echo "ci-checkout.sh: refusing a repo URL with userinfo; use SSH keys or a credential helper" >&2
    exit 2
fi

if ! [[ "${SHA}" =~ ^[0-9a-f]{40}$ ]]; then
    echo "ci-checkout.sh: sha must be a full 40-character lowercase hex commit id" >&2
    exit 2
fi

if [[ "${DEST}" != /* ]] || [[ "${DEST}" == *"/../"* ]] || [[ "${DEST}" == */.. ]] || [ "${DEST}" = / ]; then
    echo "ci-checkout.sh: dest must be an absolute path without .. segments" >&2
    exit 2
fi

if [ -e "${DEST}" ] || [ -L "${DEST}" ]; then
    echo "ci-checkout.sh: ${DEST} already exists; refusing to reuse it" >&2
    exit 2
fi

# Only remove a directory this run created: we verified it did not exist above,
# so anything present at cleanup time was made by the clone below.
CREATED=0
cleanup_on_failure() {
    rc=$?
    if [ "${rc}" -ne 0 ] && [ "${CREATED}" -eq 1 ] && [ -d "${DEST}" ]; then
        rm -rf -- "${DEST}"
    fi
    exit "${rc}"
}
trap cleanup_on_failure EXIT
CREATED=1

SSH_COMMAND="${BREAD_CI_SSH_COMMAND:-ssh -F /dev/null -oBatchMode=yes -oConnectTimeout=5 -oStrictHostKeyChecking=yes}"
CLONE_TIMEOUT="${BREAD_CI_CLONE_TIMEOUT:-300}"
if ! [[ "${CLONE_TIMEOUT}" =~ ^[1-9][0-9]*$ ]]; then
    echo "ci-checkout.sh: clone timeout must be a positive integer number of seconds" >&2
    exit 2
fi
timeout "${CLONE_TIMEOUT}s" env GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="${SSH_COMMAND}" \
    git clone --quiet --no-checkout -- "${REPO_URL}" "${DEST}"
git -C "${DEST}" -c advice.detachedHead=false checkout --quiet --detach "${SHA}"

actual="$(git -C "${DEST}" rev-parse HEAD)"
if [ "${actual}" != "${SHA}" ]; then
    echo "ci-checkout.sh: HEAD is ${actual}, expected ${SHA}" >&2
    exit 1
fi
