# Contributing

This repo is a Cargo workspace. Bakery-channel products shipped from here
are `bakery` (the ecosystem package manager) and `bread-theme` (the shared
theming crate). Shared crates that sibling apps pin — not bakery packages
of their own — are `bread-utils`, `bread-app`, `bread-onnx`,
`bread-screenshots`, and `bread-capture`. `bread-polkit` is an in-tree
session agent: it has `bread-polkit/bakery.toml` so it *can* be published,
but it is not in `registry/bread-ecosystem.toml` (unpublished — not on
the bakery index, not on the BOS ISO). Other ecosystem products
(`bread`, `breadbar`, `breadbox`, …) live in their own repos under
`Breadway/` but follow the same workflow described here. The product list
is `registry/bread-ecosystem.toml`. New GTK tools should depend on
`bread-app` instead of copying another app's bootstrap.

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
   branch operation. Install the beta build (`bakery track set beta &&
   bakery update --all`) and use it for at least 3 days. Cut `-rc.2`,
   `-rc.3`, etc. for further fixes from the new commit.
3. Once an RC has finished soaking with no open issues, tag the real
   release on the **same commit** as that RC:
   `git tag vX.Y.Z <rc-commit> && git push origin vX.Y.Z`: that's what
   triggers the signed stable release build. Its first step is the beta
   promotion gate, which rejects a stable tag that has no RC on the same
   commit or whose RC was published to beta less than 3 days earlier. See
   `docs/release-channels.md` ("Beta promotion gate").
