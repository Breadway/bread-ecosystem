# Contributing

This repo is a Cargo workspace. Bakery-channel products shipped from here
are `bakery` (the ecosystem package manager) and `bread-theme` (the shared
theming crate). Shared crates that sibling apps pin — not bakery packages
of their own — are `bread-utils`, `bread-onnx`, `bread-screenshots`, and
`bread-capture`. Other ecosystem products (`bread`, `breadbar`, `breadbox`,
…) live in their own repos under `Breadway/` but follow the same workflow
described here. The product list is `registry/bread-ecosystem.toml`.

## Branches

There is one long-lived branch: **`main`**. All day-to-day work lands here.
Every push to `main` automatically builds and publishes a **dev-track**
build for both products (see Tracks below) — use this to test your change
in a real install before cutting anything more formal.

New work — features and bug fixes alike — goes on a short-lived branch:

```
feature/<short-name>
fix/<issue-number-or-short-name>
```

Branch off `main`, open a PR/push back into `main` when ready. Short-lived
branches get deleted on merge — they never accumulate the kind of drift a
second long-lived branch does.

## The release cycle

There's no separate `beta` or release branch — "stable" and "beta" are both
just **tags** on `main`, not branches that need to be kept in sync:

1. Work accumulates on `main` via `feature/x` / `fix/x` branches. Each push
   auto-publishes a dev build for both `bakery` and `bread-theme` — install
   with `bakery track set dev` and `bakery update --all`, then fix anything
   broken with another push.
2. When you want to stabilize before a real release, tag a release
   candidate: `git tag vX.Y.Z-rc.1 && git push origin vX.Y.Z-rc.1` (push to
   both remotes). That tag alone triggers a beta-track build —
   "freezing" is just pausing pushes to `main` while you test it, not a
   branch operation. Cut `-rc.2`, `-rc.3`, etc. for further fixes.
3. Once an RC has gone without issues, tag the real release:
   `git tag vX.Y.Z && git push origin vX.Y.Z` — that's what triggers the
   signed stable release build.

**Version honesty**: bakery's compiled `--version` is
`[workspace.package] version` in the root `Cargo.toml`. The bakery
package version in the index (what `bakery list` shows) is the git tag.
Those must match at tag time — bump `workspace.package.version` to
`X.Y.Z` *before* pushing `vX.Y.Z` or `vX.Y.Z-rc.N`. Never jump a tag
(e.g. `v0.3.1` → `v0.7.1`) without that Cargo.toml bump; the resulting
binary will report the old workspace version while the index claims the
new tag.

**Note**: `bakery` and `bread-theme` share the same `v*` tag pattern
(both `release-bakery.yml` and `release-bread-theme.yml` trigger on
`tags: ['v*']`, pre-existing behavior this doc isn't changing) — a single
tag push builds and publishes a release for *both* products at once. If
you ever need to release one independently of the other, that's a real gap
worth fixing in the workflow files themselves, not something to work around
by hand.

## Tracks, from a user's perspective

```
bakery track show              # what you're currently on (defaults to stable)
bakery track set dev           # or beta, or stable
bakery update --all            # pull the latest build on your current track
```

| Track  | What it is | Published from |
|--------|-----------|-----------------|
| `stable` | The last tagged release | a `vX.Y.Z` tag |
| `beta` | Latest release candidate | a `vX.Y.Z-rc.N` tag |
| `dev` | Bleeding edge | `main`, on every push |

Dev versions are auto-computed (`X.Y.Z-dev.<timestamp>+<sha>`) from the
latest published stable tag, so they always sort as newer than what you
have installed — no manual version bumping needed. Beta versions are just
the RC tag itself (already valid semver, already sorts below the real
release it's a candidate for).

## Local development

```sh
cargo build --release -p bakery
cargo test --release -p bakery
```

`bakery`, `bread-theme`, `bread-utils`, `bread-onnx`, `bread-screenshots`,
and `bread-capture` are all workspace members. Run the same commands with
`-p bread-theme --bin bread-theme` for that crate, or `-p bread-utils
--features bread-client` for the IPC client.

## CI

- `dev-bakery.yml` / `dev-bread-theme.yml` — triggered on push to `main`.
- `rc-bakery.yml` / `rc-bread-theme.yml` — triggered on any `vX.Y.Z-rc.N`
  tag push.
- `release-bakery.yml` / `release-bread-theme.yml` — triggered on any other
  `v*` tag push, cuts the actual stable release.
- `package.yml` — publishes `bakery` to the `[breadway]` pacman repo, also
  tag-triggered.

All CI runs on a self-hosted runner; nothing runs automatically on plain
commits or PRs beyond the track builds above. See
[`docs/release-channels.md`](docs/release-channels.md) for the full policy,
including how a new product gets wired onto these tracks.

## Questions

Open an issue on this repo's Forgejo tracker.
