# Promotion overrides

Recorded exceptions to the beta promotion gate
(`scripts/check-promotion.sh`). One file per release, named
`<package>-<version>.override`:

```toml
package = "breadbar"
version = "0.4.1"
commit = "<full 40-character commit id>"
reason = "why this release skips the beta soak"
recorded_by = "who made the decision"
recorded_at = "YYYY-MM-DD"
```

A complete record lets that exact package, version and commit past a gate
rejection. The release log prints warnings naming the reason, author and
date. Anything else (wrong commit, wrong package, missing reason) fails the
release. See `docs/release-channels.md` ("Promotion overrides").

These files are history, not configuration: do not edit or delete a record
after its release ships. Add a new file for the next exception.
