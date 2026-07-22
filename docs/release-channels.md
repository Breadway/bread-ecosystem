# Release channel policy

There are two independent distribution channels in the bread ecosystem, plus
a third "neither" state for repos that aren't distributed yet. Every repo
under `Breadway/` should sit in exactly one of these three buckets, and its
`.forgejo/workflows/` directory + packaging metadata should match that
bucket exactly — no more files, no fewer.

## The two channels

**bakery channel** (`bakery install <name>`, `curl .../get | sh`, or a raw
binary download from dl.breadway.dev / the GitHub release page). A repo is on
this channel if and only if **all** of the following are true:

1. It has a `bakery.toml` at the root (or, for a multi-product repo like
   bread-ecosystem, one per product directory).
2. It has an entry in `bread-ecosystem`'s `registry/bread-ecosystem.toml`.
   `scripts/gen-index.sh` only ever looks at repos listed there — a
   `bakery.toml` that isn't backed by a registry entry is inert.
3. It has a `.forgejo/workflows/release.yml` (or a product-specific name
   like `release-bread-theme.yml` / `release-bakery.yml` for multi-product
   repos) that builds the binary, drops it under `/srv/breadway-dl/<name>/`,
   copies `bakery.toml` alongside it, regenerates `index.json` via
   `bread-ecosystem/scripts/gen-index.sh`, and uploads the same artifacts to
   a GitHub release as a fallback mirror.

All three must be present together. Two out of three is a bug, not a
partial rollout — either finish the third piece or remove the other two.

**pacman channel** (`pacman -S <name>` from the self-hosted `[breadway]`
repo, built via AUR-style `PKGBUILD`s). A repo is on this channel if and
only if:

1. It has a `PKGBUILD` under `packaging/` (either `packaging/PKGBUILD` or
   `packaging/arch/PKGBUILD` — both patterns exist in the wild, pick
   whichever a sibling repo of the same shape already uses).
2. It has a `.forgejo/workflows/package.yml` that builds the package in an
   `archlinux:latest` container and `curl -X PUT`s the resulting
   `.pkg.tar.zst` to `https://git.breadway.dev/api/packages/Breadway/arch/os`.

A repo can be on **both** channels (most GUI/daemon apps are — see
breadbar, breadbox, breadcrumbs, bread, breadpad, breadpaper), **bakery
only** (breadclip, breadmon, breadsearch, breadshot, bread-theme, bakery
itself), **pacman only** (breadlock, breadhelp — both are OS-integration
pieces where package-manager rigor matters more than a curl-script), or
**neither** (dev-only / not yet released; no bakery.toml, no PKGBUILD, no
release or package workflow — just the repo itself, e.g. breadarr today).

`bos` is a fourth, deliberately special case: it ships as an ISO, not a
binary, via its own `release-iso.yml`. It is never on either channel and
should never carry a `bakery.toml` or `PKGBUILD`.

## Build tracks (stable/beta/dev) — orthogonal to channels

Within the **bakery channel only**, a repo can additionally publish up to
three **tracks**: `stable` (the existing tag-triggered `v*` flow, unchanged),
`beta` (a frozen stabilization branch), and `dev` (published automatically on
every push to the `dev` branch). Don't confuse "track" with "channel" above —
channel is *how* a binary reaches a user (bakery vs. pacman); track is *which
build* of a bakery-channel package they get.

Each track lives in its own subtree so they never collide:

| Track  | Index URL | Artifact root | Trigger |
|---|---|---|---|
| stable | `dl.breadway.dev/index.json` | `/srv/breadway-dl/<pkg>/<ver>/` | push tag `v*` on `main` |
| beta | `dl.breadway.dev/beta/index.json` | `/srv/breadway-dl/beta/<pkg>/<ver>/` | push to branch `beta` |
| dev | `dl.breadway.dev/dev/index.json` | `/srv/breadway-dl/dev/<pkg>/<ver>/` | push to branch `dev` |

`scripts/gen-index.sh` takes a `TRACK` env var (default `stable`) to select
which subtree it reads/writes — every existing stable release workflow needs
zero changes. Dev/beta builds skip the GitHub Release upload step entirely
(no release-per-commit spam, and beta doesn't need a GitHub mirror either) —
`dl.breadway.dev` is their only distribution point.

**The full branch lifecycle** (see also `CLAUDE.md`'s Branch model section):
day-to-day work lands on `feature/<name>` or `fix/<issue>` branches, merged
into `dev`. `dev` publishes a fresh dev-track build on every push — this is
the "test for a while, fix forward with another push" loop. When `dev` has
gone roughly a week without new issues, cut `beta` fresh from `dev`'s current
tip (`git branch -f beta dev` from a clean checkout, then force-push) — this
freezes it as the stabilization target. `beta` publishes on every push the
same way `dev` does, but only `fix/<issue>` branches merged directly into
`beta` should land there afterward; `dev` keeps moving independently for the
next cycle. After roughly a month of `beta` going without new issues, merge
`beta` into `main` and push a `vX.Y.Z` tag from `main` to cut the actual
stable release (the merge itself triggers nothing — tag-push is what fires
`release.yml`). Reset `beta` fresh from `dev` again to start the next cycle.

Auto-versioning: both `dev` and `beta` compute their build version from the
latest published `vX.Y.Z` tag (via `git ls-remote --tags`, not `Cargo.toml` —
`Cargo.toml` can drift stale relative to the actual last release) plus a
`-dev.<timestamp>+<sha>` / `-beta.<timestamp>+<sha>` suffix. This is
self-healing regardless of `Cargo.toml` drift and keeps `bakery`'s semver
check (`is_newer`) meaningful — it will correctly refuse to "update" to a
build that isn't actually newer than what's installed.

Adding beta/dev to a bakery-channel repo: copy `dev-bakery.yml` /
`beta-bakery.yml` (or `bread`'s `dev-release.yml` / `beta-release.yml` if the
repo isn't part of this monorepo) from `bread-ecosystem`/`bread`, and swap
the repo/binary names the same way the checklist below describes for
`release.yml`. Also create the repo's `dev` and `beta` branches if they don't
exist yet (`git checkout -b dev main` / `git checkout -b beta dev`, push
both). Not every bakery-channel repo needs beta/dev on day one —
`gen-index.sh` silently skips any product with no release dir under a given
track's tree, same as it already does for an unreleased product on stable.

Client side: `bakery track show` / `bakery track set <stable|beta|dev>`
remembers a global track preference (`~/.local/state/bakery/installed.json`)
and validates the target track's index is reachable and signed before
switching — it never auto-reinstalls on switch, run `bakery update --all`
afterwards.

## mirror.yml is not part of this policy

Every repo previously carried its own `.forgejo/workflows/mirror.yml` doing
a `git clone --mirror` + push to GitHub with a per-repo `MIRROR_TOKEN`
secret. That pattern is being replaced ecosystem-wide by Forgejo's native
Push Mirror feature, provisioned centrally by
`bread-ecosystem/scripts/setup-push-mirrors.sh` against the live repo list
— see that script and `scripts/cleanup-old-mirror-workflows.sh`. Once the
migration is confirmed working, no repo should have a `mirror.yml` and this
document doesn't require one. Don't add `mirror.yml` to a repo that's
missing it; that gap is intentional and about to be moot everywhere.

## Checklist for adding a repo to a channel

- **Bakery**: write `bakery.toml`, add a `[[products]]` entry to
  `bread-ecosystem/registry/bread-ecosystem.toml`, copy a sibling's
  `release.yml` (prefer one with the same shape: single binary vs. binary +
  systemd service — compare against `bread/release.yml` if there's a
  service to install, `breadmon/release.yml` if not) and swap the repo
  name / binary name / `PKG_DIR`.
- **Pacman**: write `packaging/PKGBUILD` (or `packaging/arch/PKGBUILD`),
  copy a sibling's `package.yml` and swap the repo/package name and
  `system_deps`→`pacman -Syu` package list.
- Never add either file type "just in case." An unused `bakery.toml` or
  `PKGBUILD` is exactly the kind of drift this document exists to prevent
  (see the breadlock/breadarr/bos-settings history in the audit that
  produced this doc — two of those had a stray `bakery.toml` nothing
  served, one was missing the registry entry + release.yml that would have
  made an existing `bakery.toml` real).

## Current state (as of this pass)

| Repo | bakery | pacman | tracks | notes |
|---|---|---|---|---|
| bread-ecosystem (bakery product) | yes | yes | stable, beta, dev | `release-bakery.yml` recovered from a dead `.github/workflows/release.yml` that referenced a `hestia` self-hosted runner GitHub never had registered |
| bread-ecosystem (bread-theme product) | yes | no | stable, beta, dev | |
| bread, breadbar, breadbox, breadcrumbs, breadpad, breadpaper | yes | yes | stable, beta, dev | complete on all three tracks |
| breadclip, breadmon, breadsearch, breadshot | yes | no | stable, beta, dev | complete on all three tracks |
| breadlock, breadhelp | no | yes | n/a | breadlock's `bakery.toml` was removed as orphaned; its README wrongly claimed it was a registry entry |
| bos-settings | yes | yes | stable only | was missing both the registry entry and `release.yml`; both added |
| bos | no | no | n/a | ISO-only via `release-iso.yml`; had an erroneous `bakery.toml` copy-pasted from bos-settings, removed |
| breadarr | no | no | n/a | had an orphaned `bakery.toml` with no registry entry and zero workflows; removed. Not yet assigned a channel — do that deliberately when it's ready to ship, don't infer it from a stray config file |
