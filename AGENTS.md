# AGENTS.md: Repo hygiene

Scope: this file covers *repo hygiene*: branching, remotes, CI, cleanup. It is not project documentation.

Follow [`CONTRIBUTING.md`](CONTRIBUTING.md) for any git, branch, or release work. Channel/track policy lives in [`docs/release-channels.md`](docs/release-channels.md). The product list is [`registry/bread-ecosystem.toml`](registry/bread-ecosystem.toml); regenerate the README table with `scripts/gen-readme-products.sh` after editing it. Don't invent a second long-lived branch; there is only `main`. Bakery's package version **must** match `[workspace.package] version` in the root `Cargo.toml` at tag time (`bakery --version` is compiled from that field; `bakery list` reports the git tag): never push a `v*` tag without bumping Cargo.toml to the same `X.Y.Z`.

## Remotes
- `origin`: Forgejo (`git.breadway.dev`, SSH), authoritative.
- `github`: GitHub mirror. Push both when publishing.

## CI
- `.forgejo/workflows/package.yml`, `release-bakery.yml`, `release-bread-theme.yml` all trigger on `push: tags: ['v*']`, gated to skip any tag containing `-rc.`: pushing to `main` doesn't run these. Tag a release to trigger packaging.
- `dev-bakery.yml` / `dev-bread-theme.yml` trigger on `push: branches: ['main']`; `rc-bakery.yml` / `rc-bread-theme.yml` trigger on `push: tags: ['v*']` gated to *only* run for `-rc.` tags; both auto-publish a signed, auto-versioned build to `dl.breadway.dev/{dev,beta}/`. See `docs/release-channels.md` for the full track (stable/beta/dev) policy.
- `check.yml` triggers on `push: branches: ['feature/**', 'fix/**', 'main']`: lints `git.breadway.dev` dependency pins (`scripts/lint-pins.sh`; this repo's own cross-repo dependency is `bread-utils` -> `bread-shared` from `Breadway/bread`), then fmt/clippy/test (including the `gtk` feature).
- `scripts/dev-deps.sh` and `scripts/lint-pins.sh` here are the shared scripts every consumer repo (breadbar, breadbox, ...) calls from its own `check.yml`/`dev-release.yml` to build against dependency `main` branches on dev track and PR checks, while their Cargo.toml stays tag-pinned. See `docs/release-channels.md` ("Cross-repo dependency resolution"). Part of ECO-38.

## Cleanup
- Delete feature/fix branches (local + remote) once merged. Check with `git branch --merged main`.
- A `fix/audit-findings` branch and a merged `copilot/create-readme-md` branch (both local and on `origin`/`github`) were found stale and fully merged here on 2026-07-21 and removed.

## Don't
- Don't embed credentials in remote URLs: SSH or a credential helper only.
- Don't flip bakery's default install prefix. System prefix (`/usr/local` via
  `/etc/bakery/config.toml` or `BAKERY_PREFIX`) is for BOS; hermes and
  `get.sh` stay on `~/.local`. See [`bakery/README.md`](bakery/README.md).

## Persistent memory
Read `~/.knowledge.md` at the start of a task: it points to a shared memory index at `~/.claude/memory/MEMORY.md` (also used by Claude Code). Selectively read the linked entries relevant to what you're working on.
