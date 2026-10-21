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

A repo can be on **both** channels, **bakery only** (bread, breadbar,
breadbox, breadcrumbs, breadpad, breadpaper, breadclip, breadmon,
breadsearch, breadshot, breadhelp, bos-settings, bread-theme, breadcast,
breadarr, bakery itself), **pacman only** (breadlock — installs a
root-owned `/etc/pam.d/breadlock` PAM service file with no per-user
equivalent, so it can never move to bakery), or **neither** (dev-only /
not yet released). Desktop apps dropped pacman packaging; bakery-channel
install is the supported path. `bakery` still carries a leftover
`package.yml` / `packaging/arch/PKGBUILD` from when it was also published
to the `[breadway]` pacman repo.

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
| stable | `dl.breadway.dev/index.json` | `/srv/breadway-dl/<pkg>/<ver>/` | push tag `vX.Y.Z`, only once the beta gate passes |
| beta | `dl.breadway.dev/beta/index.json` | `/srv/breadway-dl/beta/<pkg>/<ver>/` | push tag `vX.Y.Z-rc.N` (mandatory stage) |
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

**The promotion ladder** (see also `CONTRIBUTING.md`): day-to-day work
lands on `feature/<name>` or `fix/<issue>` branches, merged into `main`.
Every push to `main` publishes a fresh dev-track build. That is the "test
for a while, fix forward with another push" loop, not a release.

A stable release is a three-stage promotion. No stage can be skipped:

1. **dev**. The change is merged to `main` and has a dev build. Switch to
   it with `bakery track set dev && bakery update --all` if you want it
   before a release.
2. **beta**. Tag an RC on the exact `main` commit you intend to ship:
   `git tag vX.Y.Z-rc.1 && git push origin vX.Y.Z-rc.1` (both remotes).
   `rc-release.yml` publishes it to `dl.breadway.dev/beta`. Install it with
   `bakery track set beta && bakery update --all`, run it as the real
   build for at least **3 days**, and fix problems by cutting `-rc.2`,
   `-rc.3`, and so on from the new commit. Do not tag stable while an RC
   is still under test.
3. **stable**. Tag `vX.Y.Z` on the **same commit** as the RC that finished
   soaking:
   `git tag vX.Y.Z <commit> && git push origin vX.Y.Z`. That is what fires
   `release.yml` and republishes the tested source state to the stable
   track.

"Freezing" is pausing pushes to `main` while an RC is tested, not a branch
operation. If a fix lands after the RC, it needs a new RC. Stable is never
allowed to contain code that beta never saw.

### Beta promotion gate (enforced)

Every bakery-channel `release.yml` runs
`scripts/check-promotion.sh REPO PKG TAG COMMIT` after checkout and before
it builds anything. The gate requires both:

- a `vX.Y.Z-rc.N` tag pointing at the exact commit being tagged stable, and
- a beta artifact directory for that RC at
  `/srv/breadway-dl/beta/<pkg>/<rc-version>/`, published at least 3 days
  ago, containing `bakery.toml` and at least one checksummed binary.

If either is missing, the release fails before a single artifact is built
or published, and names the RC command to run next. There is no bypass
flag. A release candidate that only exists as a tag (never published to
beta), or a stable tag cut from a newer commit than its RC, is rejected.

The gate is time-based on purpose, but a timestamp is not the whole test:
re-running an RC publish resets that version directory's time, so a rebuilt
RC starts its soak over. The 3 day floor is the minimum, not a target;
letting an RC sit until the next release is actually needed is normal.

**Hotfixes**: there is no emergency shortcut through the gate. Cut
`vX.Y.Z-rc.N` from the fix, publish it to beta, and promote after the soak.
If a situation genuinely cannot wait 3 days, that is an explicit process
decision for a human to make and record, not something CI should allow
silently.

**What counts as testing**: the beta stage exists so the build runs on a
real machine doing real work, not so `cargo test` has another audience.
Install the RC on the machine that would use the stable release, exercise
the changed behavior, and file anything broken as a breadboard issue
against the repo. An RC that was never installed has not been through beta,
even if 3 days passed.

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
