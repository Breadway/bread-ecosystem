# AGENTS.md — Repo hygiene

Scope: this file covers *repo hygiene* — branching, remotes, CI, cleanup. It is not project documentation.

Follow [`CONTRIBUTING.md`](CONTRIBUTING.md) for any git, branch, or release work. Channel/track policy lives in [`docs/release-channels.md`](docs/release-channels.md). The product list is [`registry/bread-ecosystem.toml`](registry/bread-ecosystem.toml) — regenerate the README table with `scripts/gen-readme-products.sh` after editing it. Don't invent a second long-lived branch; there is only `main`. Bakery's package version **must** match `[workspace.package] version` in the root `Cargo.toml` at tag time (`bakery --version` is compiled from that field; `bakery list` reports the git tag) — never push a `v*` tag without bumping Cargo.toml to the same `X.Y.Z`.

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
