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
three **tracks**: `stable`, `beta`, and `dev`. Don't confuse "track" with
"channel" above — channel is *how* a binary reaches a user (bakery vs.
pacman); track is *which build* of a bakery-channel package they get.

There is no per-track branch anymore — every bakery-channel repo has exactly
one long-lived branch, `main`. Tracks are driven entirely by *what you push*,
not *which branch you push to*:

| Track  | Index URL | Artifact root | Trigger |
|---|---|---|---|
| stable | `dl.breadway.dev/index.json` | `/srv/breadway-dl/<pkg>/<ver>/` | push tag `vX.Y.Z` |
| beta | `dl.breadway.dev/beta/index.json` | `/srv/breadway-dl/beta/<pkg>/<ver>/` | push tag `vX.Y.Z-rc.N` |
| dev | `dl.breadway.dev/dev/index.json` | `/srv/breadway-dl/dev/<pkg>/<ver>/` | push to branch `main` |

`scripts/gen-index.sh` takes a `TRACK` env var (default `stable`) to select
which subtree it reads/writes — this didn't need to change. Dev/beta builds
skip the GitHub Release upload step entirely (no release-per-commit spam) —
`dl.breadway.dev` is their only distribution point.

**Why no beta/dev branches**: the old model had `dev`/`beta`/`main` as three
separate branches, with `beta` cut from `dev` periodically and `main`
supposed to move forward only via a `beta` merge. In practice `main` rotted
silently in most repos — the "merge beta into main" step was a manual,
easy-to-forget action across a dozen-plus repos with no team and no
calendar enforcement, and it also collided with a real Forgejo Actions
gotcha: tag-triggered workflows resolve *which version of the workflow
YAML to run* from the repo's default branch, not the tagged commit's
branch, so a stale `main` could silently run stale release logic even when
the tag itself pointed at fresh code. Collapsing everything onto one
branch removes the class of bug entirely — there's nothing left to fall
out of sync.

**The full lifecycle** (see also `CONTRIBUTING.md`): day-to-day work lands
on `feature/<name>` or `fix/<issue>` branches, merged into `main`. `main`
publishes a fresh dev-track build on every push — this is the "test for a
while, fix forward with another push" loop. When you want to stabilize
before a real release, tag a release candidate directly off whatever
commit on `main` you're happy with: `git tag vX.Y.Z-rc.1 && git push
origin vX.Y.Z-rc.1` (both remotes). "Freezing" is just pausing pushes to
`main` while the RC gets tested, not a branch operation — cut `-rc.2`,
`-rc.3`, etc. for further fixes without needing to touch any branch. Once
an RC has gone without issues, tag the real release the same way, dropping
the `-rc.N` suffix (`vX.Y.Z`) — that's what fires `release.yml`.

Auto-versioning: `dev` computes its build version from the latest published
*stable* `vX.Y.Z` tag (via `git ls-remote --tags`, filtered to exclude any
tag containing a `-`, not `Cargo.toml` — `Cargo.toml` can drift stale
relative to the actual last release) plus a `-dev.<timestamp>+<sha>`
suffix. `beta` needs no computation at all — the RC tag itself
(`X.Y.Z-rc.N`) is already valid semver and is used as the version verbatim.
`bakery`'s semver check (`is_newer`), backed by the real `semver` crate,
already orders these correctly with zero special-casing: a prerelease
identifier sorts below the same version without one, and `dev` < `rc`
alphabetically, giving `X.Y.Z-dev... < X.Y.Z-rc.N < X.Y.Z` for the same
base version.

Adding dev/beta to a bakery-channel repo: copy `dev-bakery.yml` /
`rc-bakery.yml` (or `bread`'s `dev-release.yml` / `rc-release.yml` if the
repo isn't part of this monorepo) from `bread-ecosystem`/`bread`, and swap
the repo/binary names the same way the checklist below describes for
`release.yml`. No branch setup needed beyond the repo's single `main`. Not
every bakery-channel repo needs beta/dev on day one — `gen-index.sh`
silently skips any product with no release dir under a given track's tree,
same as it already does for an unreleased product on stable.

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
  `dev-release.yml` / `rc-release.yml` / `release.yml` trio (prefer one with
  the same shape: single binary vs. binary + systemd service — compare
  against `bread`'s if there's a service to install, `breadmon`'s if not)
  and swap the repo name / binary name / `PKG_DIR`. No branch setup beyond
  the repo's single `main`.
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
| bread-ecosystem (bakery product) | yes | yes | stable, beta, dev | single-trunk model; `release-bakery.yml` recovered from a dead `.github/workflows/release.yml` that referenced a `hestia` self-hosted runner GitHub never had registered |
| bread-ecosystem (bread-theme product) | yes | no | stable, beta, dev | single-trunk model |
| bread, breadbar, breadbox, breadcrumbs, breadpad, breadpaper | yes | no | stable, beta, dev | pacman packaging (PKGBUILD + `package.yml`) dropped — bakery-only, single-trunk model |
| breadclip, breadmon, breadsearch, breadshot | yes | no | stable, beta, dev | single-trunk model |
| breadhelp, bos-settings | yes | no | stable, beta, dev | both moved onto bakery this cycle (previously pacman-only or partially wired); single-trunk model |
| breadlock | no | yes | n/a | deliberate, permanent exception — installs a root-owned `/etc/pam.d/breadlock` PAM service file with no per-user equivalent, so it can never move to bakery |
| bos | no | no | n/a | ISO-only via `release-iso.yml`; ships via a manual local build (`build-local.sh`), not a CI track — see its own branch note below |
| breadarr | no | no | n/a | had an orphaned `bakery.toml` with no registry entry and zero workflows; removed. Not yet assigned a channel — do that deliberately when it's ready to ship, don't infer it from a stray config file |

`bos` doesn't follow the tracks table above (it has no `dev`/`beta`/`stable`
publish cadence — ISO builds are deliberate and manual) but does share the
single-`main`-branch model for the same rot-avoidance reason. It additionally
carries a `stable` branch that CI fast-forwards to whatever commit the latest
`vX.Y.Z` tag points at — a marker only, never merged into by hand, so it
can't drift the way a manually-promoted branch did before.
