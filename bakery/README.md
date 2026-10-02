# bakery

Package manager for the bread ecosystem. Usage lives in the
[repo README](../README.md).

## Install prefix

Default root is `~/.local` (bins in `~/.local/bin`, data in
`~/.local/share`). That is the plain `get.sh` path (non-BOS installs) and
must stay the default.

BOS sets a system prefix so bakery-managed desktop apps live on the `@`
root subvolume and are included in snapper/grub-btrfs snapshots:

```toml
# /etc/bakery/config.toml
prefix = "/usr/local"
```

`BAKERY_PREFIX` overrides the config file. A non-home prefix installs:

| Thing | Path |
|-------|------|
| bins | `$prefix/bin` |
| share / desktop / licenses / data | `$prefix/share/...` |
| systemd user units | `/usr/lib/systemd/user` |

Per-user state (`installed.json` and pre-update backups) stays in
`~/.local/state/bakery`. Writes that need root use `sudo -n`, then
`pkexec`. `bakery doctor` prints the active prefix.

## Snapper package transactions

System-prefix installs can opt into a numbered pre/post pair around each
`install`, `update` (including `self-update`), `remove`, or `rollback` command:

```toml
# /etc/bakery/config.toml
prefix = "/usr/local"
snapper_configs = ["root", "home"]
```

Bakery creates a pre snapshot for each configured Snapper configuration and
closes it with a post snapshot referencing its number, including when a package
command fails after partial writes. The post description records the outcome.
Missing Snapper configurations or snapshot errors produce warnings and preserve
the package command's result. A forced kill or failed post snapshot can still
leave a pre snapshot, so inspect warnings and Snapper's cleanup policy.

Snapshot commands run directly, then try non-interactive `sudo -n` when needed.
Grant Snapper access with its `ALLOW_USERS` setting or a suitably restricted
sudo rule. Bakery never requests an interactive password for snapshots.
User-prefix installs, read-only commands, and `--dry-run` create no snapshots.
Commands that find no changes may create an empty pair. `--no-hooks` disables
package `post_install` scripts, not transaction snapshots. `bakery capabilities`
reports `snapper-transactions-v1` for support and
`snapper-transactions-configured-v1` when this invocation uses an opted-in
system prefix. Update wrappers should check the configured token under the
same privilege context as the update to avoid duplicate pairs.
