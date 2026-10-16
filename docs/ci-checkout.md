# CI checkout helper (ECO-79)

Consumer check and release workflows must check out the exact commit an event
was created for, into a directory owned by the job. The shared runner runs
jobs for different branches, and a fixed `src` path lets one job replace the
tree under another (see ECO-79).

## Using the helper

`scripts/ci-checkout.sh <repo-url> <sha> <dest>` clones into `<dest>`, detaches
at `<sha>`, verifies `HEAD`, and removes `<dest>` on failure. `<dest>` must be
an absolute path that does not exist yet. The URL must carry no userinfo.

## Acquiring the helper before the checkout

The helper lives in the repo being checked out, so a workflow cannot run it
before the clone. Consumers that adopt the helper should do one of:

1. **Pinned fetch of the helper (preferred once a reviewed helper is merged):**
   fetch `scripts/ci-checkout.sh` from a pinned bread-ecosystem commit (a
   reviewed `main` SHA or a tag, never a moving branch), verify its SHA-256
   against a value committed in the consumer repo, then run it. The consumer
   keeps the expected hash in its workflow file, so a changed helper fails the
   job.
2. **Inline bootstrap:** copy the same logic into the workflow, as
   `.forgejo/workflows/check.yml` does. It must keep the SHA format check and
   the `HEAD == GITHUB_SHA` verification. Any change to the helper must be
   mirrored there, so the two cannot silently diverge.

## Cleanup

The job writes its directory to `GITHUB_ENV` (for example `ECO_CHECK_SRC`)
immediately after `mktemp -d`. A final `if: always()` step removes that path
only when it matches `${RUNNER_TEMP}/<prefix>.*` and is a directory. It runs
last, so no step loses its tree.

## Tests

`scripts/test-ci-checkout.sh` covers exact-SHA checkout after the branch
advances, two concurrent jobs on different branches, a control that reproduces
the old shared-path race, failure cleanup, userinfo rejection before network
access, and input validation.
