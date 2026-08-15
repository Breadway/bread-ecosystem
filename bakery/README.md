# bakery

Package manager for the bread ecosystem. Usage lives in the
[repo README](../README.md).

## Install prefix

Default root is `~/.local` (bins in `~/.local/bin`, data in
`~/.local/share`). That is the hermes / `get.sh` path and must stay the
default.

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
