#!/usr/bin/env python3
"""Host cache validation and retention for the shared CI launcher (ECO-102).

The CI launcher must not fall back to the root filesystem when its intended
cache mount is missing or is a different filesystem. This module provides the
fail-closed mount check, the lease record shared with retention, and a
lease-aware cleanup for CI targets.

Layout under the selected host cache root:

    <root>/targets/<key>    one target directory per product
    <root>/state/<key>.json {manifest, target, last_used}
    <root>/state/<key>.lock flock held for the duration of a build

`<key>` is 24 lowercase hex characters. That mirrors the managed Cargo target
contract in `~/.local/lib/bread-build-storage/storage.py`, so the same retention
logic can adopt CI targets without a second lease format.

Operations:
    key <product>                     print the 24-character key
    check --root PATH --uuid UUID     fail closed unless PATH is on that mount
    touch <root> <key> <target>       create or refresh the lease record
    retain <root> [--apply]           clean idle, unleased, owned targets

No operation prints a credential. The module never deletes a target that is not
under `<root>/targets`, is a symlink, has foreign ownership, or is actively
leased.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path


def key_for(product: str) -> str:
    return hashlib.sha256(("ci:" + product).encode()).hexdigest()[:24]


def mount_uuid(path: str) -> str | None:
    result = subprocess.run(
        ["findmnt", "-nro", "UUID", "--mountpoint", path],
        capture_output=True,
        text=True,
    )
    if result.returncode:
        return None
    return result.stdout.strip() or None


def check_root(root: Path, expected_uuid: str, uuid_lookup=mount_uuid) -> None:
    """Fail closed unless `root` is an owned directory on the expected mount."""

    if not expected_uuid:
        raise RuntimeError(
            "BREAD_CI_HOST_CACHE_UUID is not set; refusing an unverified cache root"
        )
    actual = uuid_lookup(str(root))
    if actual != expected_uuid:
        probe = root
        while not probe.exists() and probe != probe.parent:
            probe = probe.parent
        actual = uuid_lookup(str(probe))
    if actual != expected_uuid:
        raise RuntimeError(
            "cache root is not on the expected filesystem "
            f"(expected UUID {expected_uuid}, found {actual or 'not mounted'}); "
            "refusing to fall back to the root filesystem"
        )
    root.mkdir(parents=True, exist_ok=True)
    if uuid_lookup(str(root)) != expected_uuid:
        raise RuntimeError(
            "cache root resolved to a different filesystem after creation; refusing"
        )
    if not root.is_dir():
        raise RuntimeError(f"cache root does not exist: {root}")
    if root.is_symlink():
        raise RuntimeError(f"cache root must not be a symlink: {root}")
    if root.stat().st_uid != os.getuid():
        raise RuntimeError(f"cache root is not owned by the current user: {root}")
    if root.stat().st_mode & 0o022:
        raise RuntimeError(f"cache root is writable by another user: {root}")
    for child in ("targets", "state"):
        directory = root / child
        directory.mkdir(exist_ok=True)
        if directory.is_symlink():
            raise RuntimeError(f"{directory} must not be a symlink")
        if directory.stat().st_uid != os.getuid():
            raise RuntimeError(f"{directory} is not owned by the current user")
        if directory.stat().st_mode & 0o022:
            raise RuntimeError(f"{directory} is writable by another user")


def lease_paths(root: Path, key: str) -> tuple[Path, Path]:
    if len(key) != 24 or any(character not in "0123456789abcdef" for character in key):
        raise RuntimeError(f"invalid cache key: {key}")
    return root / "state" / (key + ".json"), root / "state" / (key + ".lock")


def touch(root: Path, key: str, target: Path) -> None:
    record_path, _ = lease_paths(root, key)
    record = {
        "manifest": "ci:" + key,
        "target": str(target),
        "last_used": time.time(),
    }
    temporary = record_path.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(record) + "\n", encoding="utf-8")
    temporary.replace(record_path)


def _owned_tree(path: Path) -> bool:
    if path.is_symlink() or not path.is_dir():
        return False
    if path.stat().st_uid != os.getuid():
        return False
    return all(
        not entry.is_symlink() and entry.lstat().st_uid == os.getuid()
        for entry in path.rglob("*")
    )


def retain(root: Path, idle_days: float, free_floor_gib: float, apply: bool) -> int:
    """Delete idle, unleased, owned CI targets. Returns the number deleted."""

    state = root / "state"
    targets = root / "targets"
    floor = int(free_floor_gib * 1024**3)
    ttl = idle_days * 86400
    deleted = 0
    for record_path in sorted(state.glob("*.json")):
        key = record_path.stem
        try:
            record = json.loads(record_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            print(f"SKIP unreadable lease record {record_path}", flush=True)
            continue
        target = Path(record.get("target", ""))
        expected = targets / key
        if target != expected or key != record_path.stem:
            print(
                f"SKIP lease record does not match its key: {record_path}", flush=True
            )
            continue
        if not expected.exists():
            continue
        if not str(expected.resolve()).startswith(str(targets.resolve()) + os.sep):
            print(f"SKIP target escapes the cache root: {expected}", flush=True)
            continue
        _, lock_path = lease_paths(root, key)
        with lock_path.open("a") as lease:
            try:
                fcntl.flock(lease.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                print(f"SKIP active lease: {expected}", flush=True)
                continue
            try:
                last_used = float(
                    json.loads(record_path.read_text(encoding="utf-8"))["last_used"]
                )
            except (OSError, ValueError, KeyError):
                print(f"SKIP unreadable last_used: {record_path}", flush=True)
                continue
            if time.time() - last_used < ttl and shutil.disk_usage(root).free >= floor:
                continue
            if not _owned_tree(expected):
                print(f"SKIP foreign ownership or symlink: {expected}", flush=True)
                continue
            print(("DELETE " if apply else "WOULD DELETE ") + str(expected), flush=True)
            if apply:
                shutil.rmtree(expected)
                record_path.unlink(missing_ok=True)
                deleted += 1
    return deleted


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="operation", required=True)

    key_parser = subparsers.add_parser("key")
    key_parser.add_argument("product")

    check_parser = subparsers.add_parser("check")
    check_parser.add_argument("--root", required=True)
    check_parser.add_argument("--uuid", required=True)

    touch_parser = subparsers.add_parser("touch")
    touch_parser.add_argument("root")
    touch_parser.add_argument("key")
    touch_parser.add_argument("target")

    retain_parser = subparsers.add_parser("retain")
    retain_parser.add_argument("root")
    retain_parser.add_argument("--apply", action="store_true")
    retain_parser.add_argument("--idle-days", type=float, default=14)
    retain_parser.add_argument("--free-floor-gib", type=float, default=100)

    args = parser.parse_args(argv)
    try:
        if args.operation == "key":
            print(key_for(args.product))
        elif args.operation == "check":
            check_root(Path(args.root), args.uuid)
        elif args.operation == "touch":
            touch(Path(args.root), args.key, Path(args.target))
        elif args.operation == "retain":
            retain(Path(args.root), args.idle_days, args.free_floor_gib, args.apply)
    except (OSError, RuntimeError) as error:
        print(f"host-cache: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
