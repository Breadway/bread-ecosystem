#!/usr/bin/env python3
"""Report bakery-channel repos whose dev work is overdue for promotion.

The script only reads checked-out repositories. Fetch origin (including tags)
before running it so origin/main and the promotion tags are current.
"""

from __future__ import annotations

import argparse
import datetime as dt
import re
import subprocess
import sys
import time
from pathlib import Path


AGE_THRESHOLD_SECONDS = 14 * 24 * 60 * 60
COMMIT_THRESHOLD = 20
PROMOTION_TAG = re.compile(r"^v\d+\.\d+\.\d+(?:-rc\.\d+)?$")


def git(repo: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def has_dev_track(repo: Path) -> bool:
    workflows = repo / ".forgejo" / "workflows"
    return workflows.is_dir() and any(
        path.name.startswith("dev-") and path.suffix in {".yml", ".yaml"}
        for path in workflows.iterdir()
    )


def promotion_tags(repo: Path, head: str) -> list[tuple[int, int, str]]:
    refs = git(
        repo,
        "for-each-ref",
        "--format=%(refname:short)%09%(creatordate:unix)",
        "refs/tags/v*",
    ).splitlines()
    candidates: list[tuple[int, int, str]] = []
    for ref in refs:
        tag, created = ref.split("\t", maxsplit=1)
        if not PROMOTION_TAG.fullmatch(tag):
            continue
        if subprocess.run(
            ["git", "-C", str(repo), "merge-base", "--is-ancestor", tag, head],
            capture_output=True,
            check=False,
        ).returncode != 0:
            continue
        ahead = int(git(repo, "rev-list", "--count", f"{tag}..{head}"))
        candidates.append((ahead, -int(created or 0), tag))
    return sorted(candidates)


def inspect_repo(repo: Path, now: int | None = None) -> dict[str, object] | None:
    """Return warning details for a stale dev track, otherwise None."""
    if not has_dev_track(repo):
        return None

    try:
        head = git(repo, "rev-parse", "origin/main")
    except subprocess.CalledProcessError as exc:
        raise ValueError("origin/main is unavailable; fetch origin first") from exc

    tags = promotion_tags(repo, head)
    if tags:
        commits_ahead, _, tag = tags[0]
        if commits_ahead == 0:
            return None
        oldest_unpromoted = git(repo, "rev-list", "--reverse", f"{tag}..{head}").splitlines()[0]
        baseline = tag
    else:
        commits_ahead = int(git(repo, "rev-list", "--count", head))
        if commits_ahead == 0:
            return None
        oldest_unpromoted = git(repo, "rev-list", "--reverse", head).splitlines()[0]
        baseline = "no beta/stable tag"

    commit_epoch = int(git(repo, "show", "-s", "--format=%ct", oldest_unpromoted))
    age_seconds = max(0, (int(time.time()) if now is None else now) - commit_epoch)
    if age_seconds < AGE_THRESHOLD_SECONDS and commits_ahead < COMMIT_THRESHOLD:
        return None

    return {
        "repo": repo.name.removesuffix("-fix-worktree"),
        "baseline": baseline,
        "commits_ahead": commits_ahead,
        "oldest_commit": oldest_unpromoted,
        "oldest_commit_date": dt.datetime.fromtimestamp(
            commit_epoch, dt.timezone.utc
        ).date().isoformat(),
        "age_days": age_seconds // (24 * 60 * 60),
        "age_triggered": age_seconds >= AGE_THRESHOLD_SECONDS,
        "commit_triggered": commits_ahead >= COMMIT_THRESHOLD,
    }


def scan(base_dir: Path, now: int | None = None) -> list[dict[str, object]]:
    warnings = []
    seen_repositories = set()
    for repo in sorted(base_dir.iterdir()):
        if not repo.is_dir() or not ((repo / ".git").is_dir() or (repo / ".git").is_file()):
            continue
        try:
            identity = git(repo, "remote", "get-url", "origin").removesuffix(".git")
        except subprocess.CalledProcessError:
            identity = repo.name.removesuffix("-fix-worktree")
        if identity in seen_repositories:
            continue
        seen_repositories.add(identity)
        try:
            warning = inspect_repo(repo, now)
        except (OSError, ValueError, subprocess.CalledProcessError) as exc:
            print(f"warning: {repo.name}: {exc}", file=sys.stderr)
            continue
        if warning is not None:
            warnings.append(warning)
    return warnings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "base_dir",
        nargs="?",
        type=Path,
        default=Path.home() / "Projects",
        help="directory containing repo checkouts (default: ~/Projects)",
    )
    args = parser.parse_args()

    warnings = scan(args.base_dir.expanduser())
    for warning in warnings:
        triggers = []
        if warning["age_triggered"]:
            triggers.append(f"oldest unpromoted commit is {warning['age_days']} days old")
        if warning["commit_triggered"]:
            triggers.append(f"{warning['commits_ahead']} commits ahead")
        print(
            f"{warning['repo']}: dev track stale since {warning['baseline']}, "
            f"{'; '.join(triggers)} (oldest unpromoted commit "
            f"{warning['oldest_commit_date']})"
        )
    if not warnings:
        print("no stale dev tracks found")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
