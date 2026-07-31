# CLAUDE.md — Repo hygiene

Scope: this file covers *repo hygiene* — branching, remotes, CI, cleanup. It is not project documentation.

This repo follows the branch/release workflow documented in `CONTRIBUTING.md`
— read and follow it for any git, branch, or release work here (the
single-trunk model, `feature/x`/`fix/x` branch naming, how RC tags work,
etc). Don't improvise a different workflow. The short version: there is one
long-lived branch, `main` — no `dev` or `beta` branch exists. `main`
auto-publishes a dev-track build on every push. "Beta" and "stable" are both
just tags, not branches: push a `vX.Y.Z-rc.N` tag to publish a beta-track
build, push a plain `vX.Y.Z` tag to cut the signed stable release.
"Freezing" for stabilization means pausing pushes to `main`, not moving a
branch. This replaced an earlier three-branch (`dev`/`beta`/`main`) model
after `main` was found to have silently rotted out of sync with `dev`/`beta`
across most repos in this ecosystem — a manual "merge beta into main
monthly" step nobody reliably did across a dozen-plus repos. Collapsing to
one branch removes the class of bug; there's nothing left that can fall out
of sync.

## Remotes
- `origin` — Forgejo (`git.breadway.dev` via Hestia, SSH) — authoritative.
- `github` — GitHub mirror. Push both when publishing.

## CI
- `.forgejo/workflows/package.yml`, `release-bakery.yml`, `release-bread-theme.yml` all trigger on `push: tags: ['v*']`, gated to skip any tag containing `-rc.` — pushing to `main` doesn't run these. Tag a release to trigger packaging.
- `dev-bakery.yml` / `dev-bread-theme.yml` trigger on `push: branches: ['main']`; `rc-bakery.yml` / `rc-bread-theme.yml` trigger on `push: tags: ['v*']` gated to *only* run for `-rc.` tags — both auto-publish a signed, auto-versioned build to `dl.breadway.dev/{dev,beta}/`. See `docs/release-channels.md` for the full track (stable/beta/dev) policy.
- No build/lint/test CI runs on ordinary commits or PRs to `main` beyond the dev-track workflow above — there's no separate lint/PR-check pipeline.

## Cleanup
- Delete feature/fix branches (local + remote) once merged. Check with `git branch --merged main`.
- A `fix/audit-findings` branch and a merged `copilot/create-readme-md` branch (both local and on `origin`/`github`) were found stale and fully merged here on 2026-07-21 and removed.

## Don't
- Don't embed credentials in remote URLs — SSH or a credential helper only.
