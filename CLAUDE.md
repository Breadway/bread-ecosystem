# CLAUDE.md — Repo hygiene

Scope: this file covers *repo hygiene* — branching, remotes, CI, cleanup. It is not project documentation.

This repo follows the branch/release workflow documented in `CONTRIBUTING.md`
— read and follow it for any git, branch, or release work here (the
dev/beta/main lifecycle, `feature/x`/`fix/x` branch naming, when to cut or
reset `beta`, etc). Don't improvise a different workflow. The short version:
`main` is tag-ready and only moves via a `beta` merge; `dev` and `beta` both
auto-publish a build on every push (dev-track / beta-track respectively);
`beta` is a frozen stabilization branch cut from `dev` roughly weekly and
promoted to `main` roughly monthly. `git branch -f beta dev` (plain
branch-pointer move) is how `beta` gets reset — never `git checkout
main`/`git merge` for this.

## Remotes
- `origin` — Forgejo (`git.breadway.dev` via Hestia, SSH) — authoritative.
- `github` — GitHub mirror. Push both when publishing.

## CI
- `.forgejo/workflows/package.yml`, `release-bakery.yml`, `release-bread-theme.yml` all trigger only on `push: tags: ['v*']` — pushing to `dev`, `beta`, or `main` doesn't run these. Tag a release to trigger packaging.
- `dev-bakery.yml` / `dev-bread-theme.yml` trigger on `push: branches: ['dev']`; `beta-bakery.yml` / `beta-bread-theme.yml` trigger on `push: branches: ['beta']` — both auto-publish a signed, auto-versioned build to `dl.breadway.dev/{dev,beta}/`. See `docs/release-channels.md` for the full three-track (stable/beta/dev) policy.
- No build/lint/test CI runs on ordinary commits or PRs to `dev`/`beta` beyond what those track workflows do — there's no separate lint/PR-check pipeline.

## Cleanup
- Delete feature/fix branches (local + remote) once merged. Check with `git branch --merged dev` / `git branch --merged main`.
- A `fix/audit-findings` branch and a merged `copilot/create-readme-md` branch (both local and on `origin`/`github`) were found stale and fully merged here on 2026-07-21 and removed.

## Don't
- Don't embed credentials in remote URLs — SSH or a credential helper only.
