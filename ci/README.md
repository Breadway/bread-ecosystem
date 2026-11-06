# Shared CI build cache

`ci/build.sh` builds a product inside the pinned Arch container. It has two
independent cache layers:

| Layer | Variable | Holds |
|---|---|---|
| Fetched dependencies | `BREAD_CI_CACHE_ROOT` | `cargo-registry/` and `cargo-git/` bind mounts. Default `$HOME/.cache/bread-ci`. |
| Generated targets | `BREAD_CI_HOST_CACHE` | Per-product target directories and their lease records. Optional. |

Do not point both variables at the same directory. A pinned source-checkout
cache that some product CI also calls `BREAD_CI_CACHE_ROOT` is a third,
unrelated concept.

## Off-root host cache

Set both variables to move generated targets off the root filesystem:

```sh
export BREAD_CI_HOST_CACHE=/mnt/media/build-cache/CI/breadway
export BREAD_CI_HOST_CACHE_UUID=1b0b601f-6762-4031-bf90-21bb4a6ff22a
ci/build.sh breadbar /path/to/checkout cargo build --release --locked
```

`build.sh` validates that the cache root is on the filesystem with exactly that
UUID before creating anything. A missing mount, a different filesystem, an
unset UUID, a symlinked root, a foreign owner or a group/world-writable root
fails closed. It never falls back to the root disk while a host cache is
configured.

The container still sees `/cargo-target` with
`CARGO_TARGET_DIR=/cargo-target`. Host layout:

```
$BREAD_CI_HOST_CACHE/targets/<key>       target directory
$BREAD_CI_HOST_CACHE/state/<key>.json    {manifest, target, last_used}
$BREAD_CI_HOST_CACHE/state/<key>.lock    flock held for the whole build
```

`<key>` is `sha256("ci:" + product)[:24]`, 24 lowercase hex characters. This is
the same 24-hex key and record shape used by
`~/.local/lib/bread-build-storage/storage.py` for managed Cargo targets, so the
same retention code can adopt CI targets without a second lease format.

The lock is held on fd 9 from before `docker build` until after the artifact
copyback. It serializes two jobs for the same product and doubles as the
retention lease. `last_used` is refreshed before the build and again after
copyback.

## Copyback and release workflows

`build.sh` still copies `/cargo-target/.` back into the checkout's `target/`
inside the same container run and before the lease is released. Workflows that
copy `src/target/release` therefore keep working; the off-root directory is the
source of truth and the copyback is the release artifact path. Do not configure
a release workflow to read `$BREAD_CI_HOST_CACHE` directly, because the key is
an implementation detail.

## Retention

`ci/host_cache.py retain` is dry-run by default. It deletes a target only when
all of these hold:

- the record points at `<root>/targets/<its own key>`;
- the lock is not held (`flock -n` succeeds);
- the target is older than `--idle-days` (default 14) or the filesystem is
  below `--free-floor-gib` (default 100);
- the directory and every entry are owned by the current user and no entry is
  a symlink.

```sh
python3 ci/host_cache.py retain "$BREAD_CI_HOST_CACHE"
python3 ci/host_cache.py retain "$BREAD_CI_HOST_CACHE" --apply
```

The host Cargo wrapper and its retention timer currently manage
`Cargo/breadway/targets`; they treat `CI/` as report-only. To let them clean CI
targets, extend that timer to scan this same `targets/` and `state/` layout, or
call this helper from it. Until that is deployed, CI retention here is manual
and the host soft budget can be exceeded by unleased external caches.

## Rollout

1. Add the two environment variables to the runner's service environment, not
   to a repository file, and reload only that runner unit.
2. Run one product with `BREAD_CI_HOST_CACHE` set and confirm the container
   sees `/cargo-target`, the target appears under `targets/<key>`, and the
   copyback still produces `target/release/...`.
3. Confirm `flock -n state/<key>.lock` fails while a build runs and succeeds
   after it.
4. Run `retain` dry-run and compare its decisions with the expected idle set.
5. Update product pins to the launcher revision that contains this change.

Existing containers pin the old inodes. A symlink or a new bind mount does not
move data for an already-running container; migrate only in a quiet window and
never remove an active mount.

## Rollback

Unset `BREAD_CI_HOST_CACHE` to return to the legacy
`$BREAD_CI_CACHE_ROOT/target-<product>` directories. Leave the off-root data in
place until a separate retention decision. Do not delete a target that is
leased or referenced by a container mount.

## Tests

```sh
python3 -m unittest ci/test_host_cache.py
```

The suite covers the key shape, expected and wrong mount UUID, missing mount,
unset UUID, symlink and writable roots, lease record updates, active-lease
exclusion, idle deletion, recent retention and records that point outside the
managed tree.
