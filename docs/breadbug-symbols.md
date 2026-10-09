# Debug symbols for breadbug

Shipped binaries are stripped. When a user sends a crash report, the frames in
it are `(module, offset, build-id)`; breadbug-server turns them back into
function, file and line using debug symbols published here, keyed by the
binary's GNU build-id. A release whose symbols were never published still
produces reports, but their stacks cannot be read or grouped across versions.

## What a release pipeline must do

1. Build the release binary with a GNU build-id. Rust on Linux links with one
   by default; confirm with `readelf -n target/release/<bin> | grep 'Build ID'`.
   Keep line tables so crashes resolve to file and line:
   ```toml
   [profile.release]
   debug = "line-tables-only"
   ```
   (Symbols are stripped from what ships, see step 3.)
2. Before `strip`, publish the symbols:
   ```sh
   scripts/publish-symbols.sh target/release/<bin> ...
   ```
   This copies only the debug sections (`objcopy --only-keep-debug`) to
   `$BREADBUG_SYMBOL_DIR/<xx>/<build-id>.debug` (default
   `/srv/breadbug-symbols`, on the same host as the `dl` store).
3. Then strip and ship as before.

`publish-symbols.sh` **fails the job** if a binary has no build-id
(`REQUIRE_BUILD_ID=0` downgrades that to a warning). Re-publishing an existing
build-id is a no-op, and nothing is ever deleted: every release that can still
be installed stays symbolicatable.

## Where the symbols go

breadbug-server reads `BREADBUG_SYMBOL_DIR` (default `<data>/symbols`) as a
read-only bind mount; see its `packaging/docker-compose.yml`. The directory is
not served publicly and contains only debug sections of our own releases.

## Checking a binary from an app

`bread_utils::bug::has_build_id()` reports whether the running executable
carries one, so an app can warn in its own diagnostics.

## Tests

`scripts/test-publish-symbols.sh` builds real binaries with and without a
build-id and checks publishing, idempotence, the failure modes and the layout
breadbug-server expects.
