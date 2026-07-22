# CLAUDE.md — Repo hygiene (local only, not committed)

Scope: this file covers *repo hygiene* — branching, remotes, CI, cleanup. It is not project documentation.

## Branch model
- `main` — release branch, always tag-ready. Don't commit directly to it.
- `dev` — integration branch. Land day-to-day work here first.
- Feature/fix work goes on short-lived branches off `dev` (`feature/x`, `fix/x`), merged back into `dev`, then `dev` → `main` when ready to release.

## Remotes
- `origin` — Forgejo (`git.breadway.dev` via Hestia, SSH) — authoritative.
- `github` — GitHub mirror. Push both when publishing.

## CI
- `.forgejo/workflows/package.yml`, `release-bakery.yml`, `release-bread-theme.yml` all trigger only on `push: tags: ['v*']` — pushing to `dev` or `main` runs nothing. Tag a release to trigger packaging.
- No build/lint/test CI runs on ordinary commits or PRs — test locally before merging to `dev`/`main`.

## Cleanup
- Delete feature/fix branches (local + remote) once merged. Check with `git branch --merged dev` / `git branch --merged main`.
- A `fix/audit-findings` branch and a merged `copilot/create-readme-md` branch (both local and on `origin`/`github`) were found stale and fully merged here on 2026-07-21 and removed.

## Don't
- Don't embed credentials in remote URLs — SSH or a credential helper only.
