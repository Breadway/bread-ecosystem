#!/usr/bin/env bash
# ci-checkout.sh: check out the exact commit a CI event was created for, into
# a directory this job owns.
#
# Consumer check/release workflows used to run `rm -rf src && git clone
# --branch "$GITHUB_REF_NAME" src`. Every job on a self-hosted runner clones
# into the same relative path, so a second job could replace the tree while
# the first job's build was still reading it (ECO-79). This helper fixes both
# halves: the directory is unique per job, and the tree is pinned to the event
# SHA instead of whatever the branch points to when the step runs.
#
# Usage:
#   scripts/ci-checkout.sh <repo-url> <sha> <dest>
#
# <repo-url> is a plain https or ssh URL. Credentials must not be embedded in
# it; authenticate through the runner's credential helper or SSH key.
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

if [[ "${REPO_URL}" =~ ^[a-zA-Z+.-]+://[^/@]*:[^/@]*@ ]]; then
    echo "ci-checkout.sh: refusing a repo URL with an embedded password" >&2
    exit 2
fi

if ! [[ "${SHA}" =~ ^[0-9a-f]{40}$ ]]; then
    echo "ci-checkout.sh: sha must be a full 40-character lowercase hex commit id" >&2
    exit 2
fi

if [ -e "${DEST}" ]; then
    echo "ci-checkout.sh: ${DEST} already exists; refusing to reuse it" >&2
    exit 2
fi

cleanup_on_failure() {
    rc=$?
    if [ "${rc}" -ne 0 ] && [ -d "${DEST}" ]; then
        rm -rf -- "${DEST}"
    fi
    exit "${rc}"
}
trap cleanup_on_failure EXIT

git clone --quiet --no-checkout -- "${REPO_URL}" "${DEST}"
git -C "${DEST}" -c advice.detachedHead=false checkout --quiet --detach "${SHA}"

actual="$(git -C "${DEST}" rev-parse HEAD)"
if [ "${actual}" != "${SHA}" ]; then
    echo "ci-checkout.sh: HEAD is ${actual}, expected ${SHA}" >&2
    exit 1
fi
