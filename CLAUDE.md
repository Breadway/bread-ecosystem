# CLAUDE.md — Repo hygiene

Scope: this file covers *repo hygiene* — branching, remotes, CI, cleanup. It is not project documentation.

## Branch model
- `main` — release branch, always tag-ready. Don't commit directly to it; the only thing that lands there is a `beta` merge (see release lifecycle below).
- `dev` — integration branch. Land day-to-day work here first. Publishes a dev-track build automatically on every push (see CI below) — this is the "push, test, fix forward with another push" loop.
- `beta` — frozen stabilization branch, cut from `dev`. Publishes a beta-track build automatically on every push, same as `dev`. While frozen, only `fix/<issue>` branches merged directly into `beta` should land there — `dev` keeps moving independently for the next cycle.
- All new work — features and bug fixes alike — goes on short-lived branches: `feature/<name>` or `fix/<issue>`. Normally these branch off `dev` and merge back into `dev`. During a beta freeze, a fix for a beta-reported issue branches off `beta` instead, merges into `beta` to unblock testers, and should also be cherry-picked/merged into `dev` so the bug doesn't quietly regress there.

## Release lifecycle
1. Work lands on `dev` via `feature/x` / `fix/x` branches. Every push to `dev` auto-publishes a dev-track build (`bakery track set dev`) — test it, fix issues with another push to `dev`.
2. Once `dev` has gone roughly **a week** without new issues, cut `beta` fresh from `dev`'s current tip: `git branch -f beta dev` (from a clean checkout — don't `git checkout main`/`git merge` for this, use a plain branch-pointer move), then force-push `beta` to both remotes. This freezes it.
3. `beta` auto-publishes on every push, same as `dev`. Anyone can file issues against it on Forgejo. Fixes land via `fix/<issue>` → `beta` (and should be forwarded into `dev` too).
4. Once `beta` has gone roughly **a month** without new issues, merge `beta` → `main`, then push a `vX.Y.Z` tag from `main` to actually cut the stable release (the merge alone triggers no CI — only the tag does). Reset `beta` fresh from `dev` again to start the next cycle.

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
