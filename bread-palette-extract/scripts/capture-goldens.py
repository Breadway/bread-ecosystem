#!/usr/bin/env python3
"""Capture golden pywal output for bread-palette-extract's test fixtures.

For each image this records what real pywal produces:
  * color1..color6 from `wal -i <img> -n -q -s -t -e` run in an isolated
    HOME/XDG_CACHE_HOME (so the user's wal cache is neither read nor
    touched), read back from that HOME's ~/.cache/wal/colors.json;
  * "wal_colors": the full ordered list pywal's default backend builds
    (pywal.backends.wal.gen_colors), which is what color0..color15 are
    derived from.
Written to tests/fixtures/golden/<stem>.json. Needs pywal and ImageMagick
at dev time only.

Usage: bread-palette-extract/scripts/capture-goldens.py IMAGE...
"""
import json
import os
import subprocess
import sys
import tempfile

from pywal.backends import wal as wal_backend

HERE = os.path.dirname(os.path.abspath(__file__))
GOLDEN = os.path.join(os.path.dirname(HERE), "tests", "fixtures", "golden")


def capture(img):
    with tempfile.TemporaryDirectory() as home:
        env = dict(os.environ, HOME=home, XDG_CACHE_HOME=os.path.join(home, ".cache"))
        subprocess.run(["wal", "-i", img, "-n", "-q", "-s", "-t", "-e"], env=env, check=True)
        with open(os.path.join(home, ".cache", "wal", "colors.json")) as f:
            colors = json.load(f)["colors"]
    golden = {f"color{i}": colors[f"color{i}"].upper() for i in range(1, 7)}
    golden["wal_colors"] = [c.upper() for c in wal_backend.gen_colors(img)]
    return golden


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    for img in sys.argv[1:]:
        stem = os.path.splitext(os.path.basename(img))[0]
        ext = os.path.splitext(img)[1].lstrip(".").lower()
        name = stem if ext == "png" else f"{stem}.{ext}"
        out = os.path.join(GOLDEN, f"{name}.json")
        with open(out, "w") as f:
            json.dump(capture(os.path.abspath(img)), f, indent=2)
            f.write("\n")
        print(out)


if __name__ == "__main__":
    main()
