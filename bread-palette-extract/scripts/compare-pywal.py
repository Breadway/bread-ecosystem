#!/usr/bin/env python3
"""Compare bread-palette-extract's native palette extractor against real pywal.

The pywal side runs exactly what pywal's default `wal` backend runs
(pywal/backends/wal.py): `magick "<img>[0]" -resize 25% -colors N
-unique-colors txt:-` for N = 16..35 until more than 16 lines come back,
then `re.search("#.{6}", line)` on every colour line. color1..color6 are
entries 8..14 of that list. The native side is the `pywal-colors` example.

Needs ImageMagick 7 (`magick`) at dev time only; bread-palette-extract itself never
runs it.

Usage:
    bread-palette-extract/scripts/compare-pywal.py [--full] IMAGE_OR_DIR...

Directories are searched recursively for png/jpg/jpeg/webp/gif/bmp. With
--full the whole colour list is compared, not just color1..6.
"""
import json
import os
import re
import subprocess
import sys

EXTS = (".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp")
HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
NEAR = 2  # per-channel tolerance for the "within delta" rate
DARK = 16  # a colour whose channels are all below this counts as near-black


def wal_colors(img):
    for n in range(16, 36):
        run = subprocess.run(
            ["magick", img + "[0]", "-resize", "25%", "-colors", str(n),
             "-unique-colors", "txt:-"],
            capture_output=True, text=True)
        if run.returncode != 0:
            return None
        lines = run.stdout.splitlines()
        if len(lines) > 16:
            return [re.search("#.{6}", line).group(0).lower() for line in lines[1:]]
    return None


def native_colors(images):
    subprocess.run(["cargo", "build", "--quiet", "--release", "-p", "bread-palette-extract",
                    "--example", "pywal-colors"], cwd=CRATE, check=True)
    target = os.environ.get("CARGO_TARGET_DIR", os.path.join(os.path.dirname(CRATE), "target"))
    exe = os.path.join(target, "release", "examples", "pywal-colors")
    out = subprocess.run([exe, *images], capture_output=True, text=True, check=True).stdout
    return {row["path"]: row for row in map(json.loads, out.splitlines())}


def rgb(h):
    return [int(h[i:i + 2], 16) for i in (1, 3, 5)]


def main():
    args = sys.argv[1:]
    full = "--full" in args
    args = [a for a in args if a != "--full"]
    images = []
    for a in args:
        if os.path.isdir(a):
            for root, _, files in os.walk(a):
                images += [os.path.join(root, f) for f in sorted(files) if f.lower().endswith(EXTS)]
        else:
            images.append(a)
    if not images:
        sys.exit(__doc__)
    native = native_colors(images)
    slots = exact = near = degenerate = 0
    total_delta = 0.0
    times = []
    for img in images:
        name = os.path.basename(img)[:32]
        w = wal_colors(img)
        row = native[img]
        o = row.get("colors")
        times.append(row["ms"])
        if w is None or o is None:
            print(f"{name:34} wal={'FAIL' if w is None else 'ok'} "
                  f"native={row.get('error') or ('none' if o is None else 'ok')}")
            continue
        if full:
            if len(w) != len(o):
                print(f"{name:34} list length differs: wal {len(w)}, native {len(o)}")
            pairs = list(zip(w, o))
        else:
            pairs = list(zip(w[8:14], o[8:14]))
        e = n = 0
        d_sum = 0.0
        for p, q in pairs:
            d = [abs(a - b) for a, b in zip(rgb(p), rgb(q))]
            e += p == q
            n += max(d) <= NEAR
            d_sum += sum(d) / 3
        slots += len(pairs)
        exact += e
        near += n
        total_delta += d_sum
        if all(max(rgb(c)) < DARK for c in o[8:14]):
            degenerate += 1
        print(f"{name:34} exact {e}/{len(pairs)}  within{NEAR} {n}/{len(pairs)}  "
              f"mean delta/ch {d_sum / len(pairs):5.2f}  {row['ms']:7.1f} ms")
        if e != len(pairs):
            print(f"{'':34} wal    {' '.join(p for p, _ in pairs)}")
            print(f"{'':34} native {' '.join(q for _, q in pairs)}")
    if slots:
        print(f"\nimages {len(images)}, slots {slots}: exact {exact} ({100 * exact / slots:.1f}%), "
              f"within delta {NEAR} {near} ({100 * near / slots:.1f}%), "
              f"mean per-channel delta {total_delta / slots:.3f}, "
              f"near-black accent sets {degenerate}")
    if times:
        print(f"native time per image: mean {sum(times) / len(times):.0f} ms, max {max(times):.0f} ms")


if __name__ == "__main__":
    main()
