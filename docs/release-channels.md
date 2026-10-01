# Release channel policy

There are two independent distribution channels in the bread ecosystem, plus
a third "neither" state for repos that aren't distributed yet. Every repo
under `Breadway/` should sit in exactly one of these three buckets, and its
`.forgejo/workflows/` directory + packaging metadata should match that
bucket exactly: no more files, no fewer.

## The two channels

**bakery channel** (`bakery install <name>`, `curl .../get | sh`, or a raw
binary download from dl.breadway.dev / the GitHub release page). A repo is on
this channel if and only if **all** of the following are true:

1. It has a `bakery.toml` at the root (or, for a multi-product repo like
   bread-ecosystem, one per product directory).
2. It has an entry in `bread-ecosystem`'s `registry/bread-ecosystem.toml`.
   `scripts/gen-index.sh` only ever looks at repos listed there; a
   `bakery.toml` that isn't backed by a registry entry is inert.
3. It has a `.forgejo/workflows/release.yml` (or a product-specific name
   like `release-bread-theme.yml` / `release-bakery.yml` for multi-product
   repos) that builds the binary, drops it under `/srv/breadway-dl/<name>/`,
   copies `bakery.toml` alongside it, regenerates `index.json` via
   `bread-ecosystem/scripts/gen-index.sh`, and uploads the same artifacts to
   a GitHub release as a fallback mirror.

All three must be present together. Two out of three is a bug, not a
partial rollout; either finish the third piece or remove the other two.

**pacman channel** (`pacman -S <name>` from the self-hosted `[breadway]`
repo, built via AUR-style `PKGBUILD`s). A repo is on this channel if and
only if:

1. It has a `PKGBUILD` under `packaging/` (either `packaging/PKGBUILD` or
   `packaging/arch/PKGBUILD`, both patterns exist in the wild, pick
   whichever a sibling repo of the same shape already uses).
2. It has a `.forgejo/workflows/package.yml` that builds the package in an
   `archlinux:latest` container and `curl -X PUT`s the resulting
   `.pkg.tar.zst` to `https://git.breadway.dev/api/packages/Breadway/arch/os`.

A repo can be on **both** channels, **bakery only** (bread, breadbar,
breadbox, breadcrumbs, breadpad, breadpaper, breadclip,
breadsearch, breadshot, breadhelp, bos-settings, bread-theme, breadcast,
breadarr, bakery itself), **pacman only** (breadlock, installs a
root-owned `/etc/pam.d/breadlock` PAM service file with no per-user
equivalent, so it can never move to bakery), or **neither** (dev-only /
not yet released). Desktop apps dropped pacman packaging; bakery-channel
install is the supported path. `bakery` still carries a leftover
`package.yml` / `packaging/arch/PKGBUILD` from when it was also published
to the `[breadway]` pacman repo.

`bos` is a fourth, deliberately special case: it ships as an ISO, not a
binary, via its own `release-iso.yml`. It is never on either channel and
should never carry a `bakery.toml` or `PKGBUILD`.

## Build tracks (stable/beta/dev): orthogonal to channels

Within the **bakery channel only**, a repo can additionally publish up to
three **tracks**: `stable`, `beta`, and `dev`. Don't confuse "track" with
"channel" above: channel is *how* a binary reaches a user (bakery vs.
pacman); track is *which build* of a bakery-channel package they get.

There is no per-track branch anymore: every bakery-channel repo has exactly
one long-lived branch, `main`. Tracks are driven entirely by *what you push*,
not *which branch you push to*:

| Track  | Index URL | Artifact root | Trigger |
|---|---|---|---|
| stable | `dl.breadway.dev/index.json` | `/srv/breadway-dl/<pkg>/<ver>/` | push tag `vX.Y.Z` |
| beta | `dl.breadway.dev/beta/index.json` | `/srv/breadway-dl/beta/<pkg>/<ver>/` | push tag `vX.Y.Z-rc.N` |
| dev | `dl.breadway.dev/dev/index.json` | `/srv/breadway-dl/dev/<pkg>/<ver>/` | push to branch `main` |

`scripts/gen-index.sh` takes a `TRACK` env var (default `stable`) to select
which subtree it reads/writes; this didn't need to change. Dev/beta builds
skip the GitHub Release upload step entirely (no release-per-commit spam);
`dl.breadway.dev` is their only distribution point.

**Why no beta/dev branches**: the old model had `dev`/`beta`/`main` as three
separate branches, with `beta` cut from `dev` periodically and `main`
supposed to move forward only via a `beta` merge. In practice `main` rotted
silently in most repos: the "merge beta into main" step was a manual,
easy-to-forget action across a dozen-plus repos with no team and no
calendar enforcement, and it also collided with a real Forgejo Actions
gotcha: tag-triggered workflows resolve *which version of the workflow
YAML to run* from the repo's default branch, not the tagged commit's
branch, so a stale `main` could silently run stale release logic even when
the tag itself pointed at fresh code. Collapsing everything onto one
branch removes the class of bug entirely: there's nothing left to fall
out of sync.

**The full lifecycle** (see also `CONTRIBUTING.md`): day-to-day work lands
on `feature/<name>` or `fix/<issue>` branches, merged into `main`. `main`
publishes a fresh dev-track build on every push; this is the "test for a
while, fix forward with another push" loop. When you want to stabilize
before a real release, tag a release candidate directly off whatever
commit on `main` you're happy with: `git tag vX.Y.Z-rc.1 && git push
origin vX.Y.Z-rc.1` (both remotes). "Freezing" is just pausing pushes to
`main` while the RC gets tested, not a branch operation; cut `-rc.2`,
`-rc.3`, etc. for further fixes without needing to touch any branch. Once
an RC has gone without issues, tag the real release the same way, dropping
the `-rc.N` suffix (`vX.Y.Z`): that's what fires `release.yml`.

## Stale dev-track warnings

The breadboard sweeper can report a bakery-channel repo when its oldest
unpromoted commit is at least 14 days old, or when its dev track is at least
20 commits ahead of its latest beta or stable promotion.
`scripts/check-stale-tracks.py` checks repos with a `dev-*.yml` workflow under
a directory of local checkouts (by default `~/Projects`). It compares each
`origin/main` with the nearest reachable version tag matching `vX.Y.Z` or
`vX.Y.Z-rc.N`. If there is no promotion tag, it measures age and commit count
from the repository's first commit. Fetch each repo, including tags, before
running the check so its refs are current.

The check is read-only. It prints warnings for maintainers to review; it does
not create tags, publish artifacts, or change repositories. A stale warning is
a prompt to decide whether to promote, not a release action.

### Breadboard sweeper integration

At the end of each normal ecosystem sweep, after fetching the repositories,
run the checker against the local checkout directory:

```sh
python3 ~/Projects/bread-ecosystem/scripts/check-stale-tracks.py ~/Projects
```

Copy any output into a separate **stale-track warnings** section of the sweep
report, including the repository, promotion tag, age, commit count, and the
threshold reached. Keep these warnings report-only: do not file backlog issues,
create tags, or publish artifacts. If there are no warnings, report that no
stale dev tracks were found.

```sh
python3 -m unittest scripts/test_check_stale_tracks.py
```

Auto-versioning: `dev` computes its build version from the latest published
*stable* `vX.Y.Z` tag (via `git ls-remote --tags`, filtered to exclude any
tag containing a `-`, not `Cargo.toml` (`Cargo.toml` can drift stale
relative to the actual last release)) plus a `-dev.<timestamp>+<sha>`
suffix. `beta` needs no computation at all: the RC tag itself
(`X.Y.Z-rc.N`) is already valid semver and is used as the version verbatim.
`bakery`'s semver check (`is_newer`), backed by the real `semver` crate,
already orders these correctly with zero special-casing: a prerelease
identifier sorts below the same version without one, and `dev` < `rc`
alphabetically, giving `X.Y.Z-dev... < X.Y.Z-rc.N < X.Y.Z` for the same
base version.

**Bakery package version honesty**: `bakery --version` is compiled from
this repo's `[workspace.package] version` (`CARGO_PKG_VERSION`); `bakery
list` reports the *tagged* package version from the index. Those two must
match at tag time: set `workspace.package.version` to `X.Y.Z` *before*
pushing `vX.Y.Z` or `vX.Y.Z-rc.N`, and never jump a git tag without that
Cargo.toml bump. The `v0.3.1` → `v0.7.1` tag jump that left Cargo.toml at
`0.3.1` is the bug this rule exists to prevent. Dev-track auto-versioning
keys off the latest stable tag rather than Cargo.toml so a stale
workspace version cannot publish a dev build that sorts *older* than
installed bakery; that fallback is not permission to leave the workspace
version stale.

Adding dev/beta to a bakery-channel repo: copy `dev-bakery.yml` /
`rc-bakery.yml` (or `bread`'s `dev-release.yml` / `rc-release.yml` if the
repo isn't part of this monorepo) from `bread-ecosystem`/`bread`, and swap
the repo/binary names the same way the checklist below describes for
`release.yml`. No branch setup needed beyond the repo's single `main`. Not
every bakery-channel repo needs beta/dev on day one; `gen-index.sh`
silently skips any product with no release dir under a given track's tree,
same as it already does for an unreleased product on stable.

Client side: `bakery track show` / `bakery track set <stable|beta|dev>`
remembers a global track preference (`~/.local/state/bakery/installed.json`)
and validates the target track's index is reachable and signed before
switching; it never auto-reinstalls on switch, run `bakery update --all`
afterwards.

## mirror.yml is not part of this policy

Every repo previously carried its own `.forgejo/workflows/mirror.yml` doing
a `git clone --mirror` + push to GitHub with a per-repo `MIRROR_TOKEN`
secret. That pattern is being replaced ecosystem-wide by Forgejo's native
Push Mirror feature, provisioned centrally by
`bread-ecosystem/scripts/setup-push-mirrors.sh` against the live repo list;
see that script and `scripts/cleanup-old-mirror-workflows.sh`. Once the
migration is confirmed working, no repo should have a `mirror.yml` and this
document doesn't require one. Don't add `mirror.yml` to a repo that's
missing it; that gap is intentional and about to be moot everywhere.

## Cross-repo dependency resolution: tag pins vs. dev-track main

A change that spans repos (bread-theme in bread-ecosystem plus a consumer
in breadbar, say) used to need a committed `rev`/`branch`/`path` pin to
test together before either side was tagged, which is exactly the kind of
pin that leaks toward a release if someone forgets to revert it. The
model below removes that need entirely:

- **Cargo.toml always pins a `git.breadway.dev` dependency to a release
  tag** (`tag = "vX.Y.Z"`), in every repo, on every branch, at all times.
  Never `rev`, never `branch`, never a bare `path` standing in for one of
  these dependencies. `scripts/lint-pins.sh` enforces this in CI (see
  below); nothing about a dev-track build should ever require relaxing it
  in the committed manifest.
- **Beta (`vX.Y.Z-rc.N`) and stable (`vX.Y.Z`) builds are unchanged**: they
  build straight from the tagged Cargo.toml with `cargo build --locked`,
  exactly as before. This is the whole reason the tag pin can stay
  permanent: released builds never need anything else.
- **Dev track (push to `main`) and PR/branch checks resolve every
  `git.breadway.dev` dependency to that dependency repo's own `main`**,
  at build time only, via `scripts/dev-deps.sh` (this repo) and a
  generated `.cargo/config.toml`. Nothing is committed: the script runs as
  a CI step, patches the checkout in place, and the build after it runs
  without `--locked`. See "Reproducing a dev build locally" below to run
  the same thing on a workstation.
- **Release order for a cross-repo change**: tag the dependency repo first
  (e.g. bread-ecosystem's next bread-theme release), then bump the
  consumer's Cargo.toml tag pins in an ordinary PR (`cargo update -p
  bread-theme` against the new tag, commit the Cargo.toml + Cargo.lock
  diff), then tag the consumer. Dev-track builds already picked up the
  change on both sides the moment each `main` got the merge; the tag bump
  is purely to move the *release* pin forward, not to unblock testing.

### `scripts/dev-deps.sh`

Run from an app's checkout root (usually the first CI step after
checkout, before the build step). For every `git.breadway.dev` dependency
declared anywhere in the app's Cargo.toml manifests, it:

1. Shallow-clones that dependency repo's `main` into a sibling directory
   (`../<repo-name>` by default; override the parent directory with
   `DEV_DEPS_DIR` if you need the clone somewhere else). Every consumer's
   `dev-release.yml`/`check.yml` should leave this at its default: `git
   clone/fetch` failures are retried automatically (3 attempts, backoff),
   and `ci/build.sh`'s docker wrapper (see below) already makes the
   default sibling location visible inside the container. Do **not**
   force `DEV_DEPS_DIR` to a path *inside* the checkout as a workaround
   for a docker-visibility problem: see the "workspace-field inheritance"
   pitfall below.
2. Writes a `.cargo/config.toml` `[patch]` block pointing each affected
   crate at its clone.
3. Runs `cargo update -p <crate>` for the patched crates so Cargo.lock
   matches.
4. Records each dependency repo's resolved SHA to `dev-deps.lock.txt`
   (printed and written to the app root) so a dev build is reproducible.

A clone directory the script created itself is marked and refreshed in
place on a later run (`git fetch --depth 1 origin main && git reset --hard
FETCH_HEAD`). A directory that already existed before the script ran
first (a developer's own sibling checkout, per "Local development" below)
is reused exactly as-is and is never fetched, reset, or otherwise mutated.
The generated `.cargo/config.toml` carries a sentinel comment identifying
it as CI-owned; the script refuses to overwrite a `.cargo/config.toml` it
didn't generate unless `--force` is passed. Run `scripts/dev-deps.sh
--help` for the full flag/env var reference, or `--check` for a dry run
that reports the plan without touching anything.

### `scripts/lint-pins.sh`

Shared CI guard, called from every consumer repo's `check.yml`: fails if
any `git.breadway.dev` dependency in any of the repo's Cargo.toml files
uses `rev` or `branch` instead of `tag`, or if a path dependency's target
resolves outside the repo (the local dev-loop path-patch pattern leaking
into a committed manifest, as opposed to an ordinary intra-workspace
`path = "../sibling-crate"` reference within the same repo, which is left
alone). Run `scripts/lint-pins.sh --root <path>` directly to check a repo
other than the current directory.

### `ci/build.sh` and the sibling-clone / Cargo-workspace pitfall

The shared, dockerized CI build script (`ci/build.sh` here, vendored by
every GTK4 consumer's own `ci/build.sh` wrapper, pinned by commit via
`ci/bread-ecosystem.rev`) mounts the checkout's **parent** directory at
`/workspace` (with the container's cwd set to `/workspace/<checkout>`),
not the checkout alone. This exists specifically so `scripts/dev-deps.sh`'s
default sibling clone is visible inside the container without needing to
live inside the checkout.

Earlier revisions of this pattern (breadbar's pilot) instead forced
`DEV_DEPS_DIR` to a path *inside* the checkout, because at the time
`ci/build.sh` only bind-mounted the checkout root. That workaround breaks
the moment the consumer is itself a Cargo workspace (any repo with a
top-level `[workspace]`, not just a single crate under one `[package]`):
Cargo attributes a dependency clone nested inside the checkout's own
workspace directory tree to the *outer* workspace for `foo.workspace =
true` field-inheritance purposes, instead of discovering the clone's own
nested `[workspace]` root, and fails with `workspace.package.foo was not
defined` the instant the outer workspace doesn't define that field. This
surfaced rolling ECO-38 out to breadbox (a workspace) after the pilot
(breadbar, a single crate, never hit it). The fix was to mount the
checkout's parent instead of the checkout itself, which is now the
standard shape for every consumer using the docker wrapper; there's no
longer any reason to override `DEV_DEPS_DIR` at all.

### Local development

The gitignored `.cargo/config.toml` path-patch convention described in
each repo's CONTRIBUTING.md is now a **developer convenience only**; CI
never depends on it and never reads it. It works exactly as before, and
`scripts/dev-deps.sh` will leave a checkout set up that way alone (see the
"reused as-is" behavior above) if you point `DEV_DEPS_DIR` at its parent
directory. To reproduce what a dev-track build or PR check does, from an
app's checkout:

```sh
../bread-ecosystem-checkout-parent/scripts/dev-deps.sh --root .
cargo build --release   # no --locked: the patched deps are on main
```

(or invoke the script from wherever you have bread-ecosystem checked out,
or cloned fresh for the purpose; it only needs its own file, no other
ecosystem tooling). `git diff Cargo.lock` afterward will show the
temporary path-patched entries; discard it (`git checkout -- Cargo.lock`)
along with the generated `.cargo/config.toml` when you're done, the same
cleanup as today's local convention already requires.

### Rolling this model out to a new batch of repos: runner load

Merging the dev-deps wiring into several bakery-channel repos in quick
succession (as the ECO-38 rollout did) fires that many first-time
`dev-release.yml` runs on the self-hosted runner back to back. Several of
those runs failed fast (well under a minute, no diagnosable cause via the
Forgejo API, which exposes no job-log endpoint) while a clean local
reproduction of the exact same `dev-deps.sh` + build commands succeeded
every time, including for products whose build is otherwise unremarkable.
The runner appears to serialize/queue jobs rather than truly run them in
parallel, and check.yml + dev-release.yml on the *same* push (for repos
whose check.yml also triggers on `main`) queue as two jobs at once. Retried
individually via a follow-up empty-commit PR per repo, spaced apart, most
came back green; a few kept failing on retry too. Since local reproduction
never reproduces it, this looks like runner-side resource pressure (disk,
docker layer cache turnover, or similar) rather than anything in the CI
wiring itself. If you're rolling this pattern out to several more repos at
once, merge and verify them one at a time rather than back to back, and if
a fresh dev-release run fails with no obvious cause, reproduce locally
first (see above) before assuming the wiring is wrong.

## Checklist for adding a repo to a channel

- **Bakery**: write `bakery.toml`, add a `[[products]]` entry to
  `bread-ecosystem/registry/bread-ecosystem.toml`, copy a sibling's
  `dev-release.yml` / `rc-release.yml` / `release.yml` trio (prefer one with
  the same shape: single binary vs. binary + systemd service, compare
  against `bread`'s if there's a service to install, `breadshot`'s if not)
  and swap the repo name / binary name / `PKG_DIR`. No branch setup beyond
  the repo's single `main`.
- **Pacman**: write `packaging/PKGBUILD` (or `packaging/arch/PKGBUILD`),
  copy a sibling's `package.yml` and swap the repo/package name and
  `system_deps`→`pacman -Syu` package list.
- Never add either file type "just in case." An unused `bakery.toml` or
  `PKGBUILD` is exactly the kind of drift this document exists to prevent
  (see the breadlock/breadarr/bos-settings history in the audit that
  produced this doc; two of those had a stray `bakery.toml` nothing
  served, one was missing the registry entry + release.yml that would have
  made an existing `bakery.toml` real).

## Current state (as of this pass)

| Repo | bakery | pacman | tracks | notes |
|---|---|---|---|---|
| bread-ecosystem (bakery product) | yes | leftover `package.yml` | stable, beta, dev | bakery-channel (`curl -fsSL https://get.breadway.dev \| sh`) is the supported install; `package.yml` + `packaging/arch/PKGBUILD` remain from when bakery was also published to `[breadway]` |
| bread-ecosystem (bread-theme product) | yes | no | stable, beta, dev | single-trunk model |
| bread, breadbar, breadbox, breadcrumbs, breadpad, breadpaper | yes | no | stable, beta, dev | pacman packaging (PKGBUILD + `package.yml`) dropped: bakery-only, single-trunk model |
| breadclip, breadsearch, breadshot | yes | no | stable, beta, dev | single-trunk model |
| breadmon | no | no | n/a | deprecated; source and historical releases remain available, but it is no longer in the registry or BOS image |
| breadhelp, bos-settings | yes | no | stable, beta, dev | bakery-channel desktop/settings apps; not pacman |
| breadlock | no | yes | n/a | deliberate, permanent exception: installs a root-owned `/etc/pam.d/breadlock` PAM service file with no per-user equivalent, so it can never move to bakery |
| bos | no | no | n/a | ISO-only via `release-iso.yml`; ships via a manual local build (`build-local.sh`), not a CI track; see its own branch note below |
| breadcast | yes | no | stable, beta, dev | bakery product; not included in the BOS ISO |
| breadarr | yes | no | stable, beta, dev | bakery product; homelab, not shipped on BOS |

`bos` doesn't follow the tracks table above (it has no `dev`/`beta`/`stable`
publish cadence (ISO builds are deliberate and manual) but does share the
single-`main`-branch model for the same rot-avoidance reason. It additionally
carries a `stable` branch that CI fast-forwards to whatever commit the latest
`vX.Y.Z` tag points at, a marker only, never merged into by hand, so it
can't drift the way a manually-promoted branch did before.
