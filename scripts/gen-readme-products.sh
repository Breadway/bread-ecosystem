#!/usr/bin/env bash
# Rewrite the marked Products table in README.md from
# registry/bread-ecosystem.toml (the source of truth).
#
# Markers (must exist in README.md):
#   <!-- gen-readme-products:start -->
#   ...generated markdown...
#   <!-- gen-readme-products:end -->
#
# Optional per-product `notes` in the registry is appended to the
# description after an em-dash (used for "homelab, not BOS" / "not in ISO").
#
# Usage: scripts/gen-readme-products.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REGISTRY="${SCRIPT_DIR}/registry/bread-ecosystem.toml"
README="${SCRIPT_DIR}/README.md"
START="<!-- gen-readme-products:start -->"
END="<!-- gen-readme-products:end -->"

if [[ ! -f "${REGISTRY}" ]]; then
    echo "error: registry not found at ${REGISTRY}" >&2
    exit 2
fi
if [[ ! -f "${README}" ]]; then
    echo "error: README not found at ${README}" >&2
    exit 2
fi

python3 - "${REGISTRY}" "${README}" "${START}" "${END}" <<'PY'
import sys
from pathlib import Path

try:
    import tomllib
except ImportError:  # pragma: no cover — 3.11+ is required
    import tomli as tomllib  # type: ignore

registry_path, readme_path, start, end = sys.argv[1:]

with open(registry_path, "rb") as f:
    registry = tomllib.load(f)

products = registry.get("products") or []
if not products:
    print("error: registry has no [[products]]", file=sys.stderr)
    sys.exit(1)

lines = ["| Package | Description |", "|---------|-------------|"]
for product in products:
    name = product["name"]
    desc = str(product.get("description") or "").replace("|", "\\|")
    notes = str(product.get("notes") or "").replace("|", "\\|")
    if notes:
        desc = f"{desc} — {notes}"
    lines.append(f"| `{name}` | {desc} |")
table = "\n".join(lines)

readme = Path(readme_path)
text = readme.read_text()
start_at = text.find(start)
end_at = text.find(end)
if start_at < 0 or end_at < 0 or end_at < start_at:
    print(
        f"error: README.md is missing markers {start!r} / {end!r}",
        file=sys.stderr,
    )
    sys.exit(1)

rewritten = text[:start_at] + start + "\n\n" + table + "\n\n" + end + text[end_at + len(end):]
if rewritten != text:
    readme.write_text(rewritten)
    print(f"updated {readme_path} ({len(products)} products)")
else:
    print(f"{readme_path} already matches the registry ({len(products)} products)")
PY
