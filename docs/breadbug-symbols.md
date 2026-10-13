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
(`REQUIRE_BUILD_ID=0` downgrades that to a warning). It also fails the job if a
binary has no usable line tables: the input and the extracted file must both
have non-empty `.debug_info` and `.debug_line` sections. A stripped binary
therefore exits non-zero and writes nothing. Re-publishing a build-id whose
stored file is usable is a no-op, and nothing valid is ever deleted: every
release that can still be installed stays symbolicatable. A stored file for the
same build-id that is unusable (empty, stripped, or carrying another build-id)
is replaced atomically by a valid one. The temp file is created in the shard
directory and renamed into place.

Binutils (`readelf`, `objcopy`, `objdump`) must be on the publishing host. The
script checks for them and fails with a clear message if they are missing.

## Private store and permissions

The store contains debug information for our own releases, which exposes source
paths and internal layout. It must not be readable by other local users, and
the script enforces this rather than trusting the caller's umask.

- The script sets `umask 027` and creates directories with mode `0750` and
  files with mode `0640`, regardless of the caller's umask.
- Only the owner and the owning group have access. Others have none, so an
  unprivileged user cannot traverse into the store.
- Files are installed with `chmod 0640`, and their group is set to the group of
  their shard directory, so the reader group can read them.
- An existing store root or shard directory is never changed by the script. If
  it is a symlink, or grants others any access, or is group-writable, the script
  refuses with the exact `chmod` needed. It does not recurse into caller-owned
  trees.
- An existing artifact that the script skips as already published has its mode
  corrected to `0640`.

Why `0750` and `0640` and not `0700` and `0600`: breadbug-server runs as a
different account from the release runner, and must read every file. Group
read is the narrowest grant that lets it do that. Nothing needs group write,
because the runner owns the store and is the only writer.

### Provisioning

Create a dedicated group that holds exactly the writer (the release runner
account) and the reader (the breadbug-server account), then:

```sh
groupadd --system breadbug-symbols
usermod -aG breadbug-symbols <runner-user>
usermod -aG breadbug-symbols <breadbug-server-user>
install -d -o <runner-user> -g breadbug-symbols -m 2750 /srv/breadbug-symbols
```

The runner must own the root, because the script only writes into directories
it can write, and the root must not be group-writable. The leading `2` is the
setgid bit: shard directories the script creates inherit the `breadbug-symbols`
group, so the reader can enter them and read the files. The script creates the
shard directories with mode `0750`.

breadbug-server reads `BREADBUG_SYMBOL_DIR` (default `<data>/symbols`) as a
read-only bind mount; see its `packaging/docker-compose.yml`. Its container must
run with the `breadbug-symbols` group (supplementary group) so the `0640`
files are readable. The directory is not served publicly and contains only
debug sections of our own releases.

## Checking a binary from an app

`bread_utils::bug::has_build_id()` reports whether the running executable
carries one, so an app can warn in its own diagnostics.

## Tests

`scripts/test-publish-symbols.sh` builds real binaries with rustc and runs in
CI (`.forgejo/workflows/check.yml`) on every feature and fix push. It checks:

- directory and file modes under umask 022 and umask 000 (0750, 0750, 0640);
- that a loose-mode existing artifact is corrected, not skipped;
- that an unsafe pre-existing store (0755, or group-writable) is refused and
  left unchanged;
- that a stripped input is rejected with a clear error, writes nothing, and
  does not block other good binaries in the same run;
- that stale or unusable artifacts (empty, stripped, wrong build-id) for the
  same build-id are replaced by valid ones;
- that the original, stripped and debug files share one build-id;
- that `addr2line` resolves `known_fn` to `a.rs:1` from the published artifact;
- idempotence, the no-build-id failure, `REQUIRE_BUILD_ID=0`, missing files,
  missing arguments and a relative store path.
