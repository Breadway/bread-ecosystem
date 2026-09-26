#!/usr/bin/env bash
# lint-pins.sh: fail if any git.breadway.dev dependency in this repo's
# Cargo.toml files uses `rev`, `branch`, or a bare `path` instead of `tag`.
#
# Tag pins are the only thing that should ever be committed (see
# docs/release-channels.md): dev-track resolution to `main` happens at
# build time via scripts/dev-deps.sh and a gitignored .cargo/config.toml,
# never by editing Cargo.toml itself. This script is the CI guard for that
# rule, shared so every consumer repo (breadbar, breadbox, ...) can call it
# instead of re-implementing the check.
#
# Usage:
#   scripts/lint-pins.sh [--root <repo-root>]
#
# Exit status: 0 if every git.breadway.dev dependency is tag-pinned, 1 if
# any is not (each offender is printed), 2 on usage error.
#
# Requires: python3 (tomllib, stdlib since 3.11).
set -euo pipefail

ROOT="."
while [ $# -gt 0 ]; do
    case "$1" in
        --root)
            ROOT="$2"
            shift 2
            ;;
        -h|--help)
            sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

ROOT="$(cd "${ROOT}" && pwd)"

mapfile -t MANIFESTS < <(find "${ROOT}" \
    -name Cargo.toml \
    -not -path '*/target/*' \
    -not -path '*/.dev-deps/*' \
    -not -path '*/.git/*' \
    | sort)

if [ "${#MANIFESTS[@]}" -eq 0 ]; then
    echo "lint-pins: no Cargo.toml files found under ${ROOT}" >&2
    exit 2
fi

FAILED=0

for manifest in "${MANIFESTS[@]}"; do
    result="$(python3 - "${manifest}" "${ROOT}" <<'PYEOF'
import os, sys, tomllib

path = sys.argv[1]
root = os.path.realpath(sys.argv[2])
manifest_dir = os.path.dirname(os.path.realpath(path))

with open(path, "rb") as f:
    doc = tomllib.load(f)

dep_tables = []
for key in ("dependencies", "dev-dependencies", "build-dependencies"):
    if key in doc:
        dep_tables.append(doc[key])
for tgt in doc.get("target", {}).values():
    for key in ("dependencies", "dev-dependencies", "build-dependencies"):
        if key in tgt:
            dep_tables.append(tgt[key])
if "workspace" in doc and "dependencies" in doc["workspace"]:
    dep_tables.append(doc["workspace"]["dependencies"])

def escapes_repo(rel_path):
    target = os.path.realpath(os.path.join(manifest_dir, rel_path))
    return target != root and not target.startswith(root + os.sep)

problems = []
for table in dep_tables:
    for name, spec in table.items():
        if not isinstance(spec, dict):
            continue
        git = spec.get("git", "")
        is_ecosystem_git = isinstance(git, str) and "git.breadway.dev" in git
        if is_ecosystem_git:
            if "rev" in spec:
                problems.append(f"{name}: git dep pinned by rev instead of tag ({git})")
            elif "branch" in spec:
                problems.append(f"{name}: git dep pinned by branch instead of tag ({git})")
            elif "tag" not in spec:
                problems.append(f"{name}: git dep to {git} has neither tag, rev, nor branch")
        elif "path" in spec and escapes_repo(spec["path"]):
            # A path dependency that stays inside this repo is a normal
            # workspace-member reference. One that points outside the repo
            # root is the local dev-loop pattern (see CONTRIBUTING.md's
            # .cargo/config.toml convention) leaking into a committed
            # Cargo.toml in place of a tagged git dependency.
            problems.append(f"{name}: path dependency ({spec['path']}) escapes this repo; use a tagged git dependency instead of a local path override")

for p in problems:
    print(p)
PYEOF
)"
    if [ -n "${result}" ]; then
        while IFS= read -r line; do
            echo "::error file=${manifest}::${line}"
        done <<< "${result}"
        FAILED=1
    fi
done

if [ "${FAILED}" -eq 1 ]; then
    echo "lint-pins: one or more git.breadway.dev dependencies are not tag-pinned" >&2
    exit 1
fi

echo "lint-pins: all git.breadway.dev dependencies across ${#MANIFESTS[@]} manifest(s) are tag-pinned"
