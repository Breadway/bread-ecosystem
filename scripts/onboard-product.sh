#!/usr/bin/env bash
# onboard-product.sh — register a new product in registry/bread-ecosystem.toml.
#
# This is the one step in bringing a product under bakery that's a genuine
# write action; everything else (bakery.toml, CI workflows, the
# BAKERY_MINISIGN_SEC_KEY_PATH Actions secret) is either copied from an
# existing product repo or diagnosed by scripts/doctor-channels.sh, which
# this script runs at the end so nothing gets missed the way breadcast's
# missing secret did.
#
# Usage:
#   scripts/onboard-product.sh <name> <owner/repo> <description>
#
# Example:
#   scripts/onboard-product.sh breadcast Breadway/breadcast \
#     "Cast your screen to any Chromecast/Google TV or DLNA renderer — daemon + GTK4 popup"

set -euo pipefail

if [[ $# -ne 3 ]]; then
    echo "usage: $0 <name> <owner/repo> <description>" >&2
    exit 2
fi

NAME="$1"
REPO="$2"
DESCRIPTION="$3"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REGISTRY="${SCRIPT_DIR}/registry/bread-ecosystem.toml"

if python3 -c "
import tomllib, sys
with open('${REGISTRY}', 'rb') as f:
    d = tomllib.load(f)
sys.exit(0 if any(p['name'] == '${NAME}' for p in d['products']) else 1)
"; then
    echo "${NAME} is already registered in ${REGISTRY}, skipping"
else
    cat >> "${REGISTRY}" <<EOF

[[products]]
name = "${NAME}"
repo = "${REPO}"
description = "${DESCRIPTION}"
EOF
    echo "added ${NAME} (${REPO}) to ${REGISTRY}"
fi

echo
echo "running doctor-channels.sh to check bakery.toml / CI workflows / signing secret are all in place for ${NAME}..."
bash "${SCRIPT_DIR}/scripts/doctor-channels.sh" || true
