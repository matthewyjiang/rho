#!/usr/bin/env python3
"""Block until a pullfrog watch event matches, then exit.

Prints the matching JSON event on stdout. Progress and the last cursor
go to stderr. Resume with --since <cursor> after a timeout or restart.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
from typing import Any

# Named waits: exit on any of these (kind, field, value).
PRESETS: dict[str, tuple[tuple[str, str, str], ...]] = {
    "approval": (
        ("review", "state", "approved"),
        ("review", "state", "changes_requested"),
        ("pr", "action", "closed"),
        ("pr", "action", "merged"),
        ("check", "conclusion", "failure"),
    ),
    "ci": (
        ("check", "conclusion", "failure"),
        ("check", "conclusion", "success"),
        ("pr", "action", "closed"),
        ("pr", "action", "merged"),
    ),
    "merged": (
        ("pr", "action", "closed"),
        ("pr", "action", "merged"),
    ),
    "review": (
        ("review", "state", "approved"),
        ("review", "state", "changes_requested"),
        ("pr", "action", "closed"),
        ("pr", "action", "merged"),
    ),
}


def parse_until(raw: str) -> tuple[tuple[str, str, str], ...]:
    if raw in PRESETS:
        return PRESETS[raw]
    rules: list[tuple[str, str, str]] = []
    for part in raw.split(","):
        part = part.strip()
        if not part:
            continue
        if part in PRESETS:
            rules.extend(PRESETS[part])
            continue
        kind, _, value = part.partition(":")
        if not kind or not value:
            raise SystemExit(
                f"bad --until item {part!r}: use a preset ({', '.join(PRESETS)}) "
                "or kind:value (review:approved, check:failure, pr:merged)"
            )
        field = {
            "review": "state",
            "check": "conclusion",
            "pr": "action",
            "review_thread": "action",
            "review_comment": "action",
            "comment": "action",
        }.get(kind, "action")
        rules.append((kind, field, value.lower()))
    if not rules:
        raise SystemExit("empty --until")
    return tuple(rules)


def event_matches(event: dict[str, Any], rules: tuple[tuple[str, str, str], ...]) -> bool:
    kind = str(event.get("kind") or "")
    data = event.get("data")
    if not isinstance(data, dict):
        data = {}
    for rule_kind, field, value in rules:
        if kind != rule_kind:
            continue
        got = data.get(field)
        if got is None and field == "action":
            got = event.get("action")
        if str(got or "").lower() == value:
            return True
    return False


def watch_command(pr: int, repo: str | None, since: str | None) -> list[str]:
    runner = "bunx" if shutil.which("bunx") else "npx"
    if not shutil.which(runner):
        raise SystemExit("need bunx or npx to run pullfrog watch")
    cmd = [runner, "pullfrog", "watch"]
    if repo:
        cmd.append(repo)
    cmd.extend(["--pr", str(pr)])
    if since:
        cmd.extend(["--since", since])
    return cmd


def run(pr: int, until: str, repo: str | None, since: str | None) -> int:
    rules = parse_until(until)
    cmd = watch_command(pr, repo, since)
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        text=True,
    )
    assert proc.stdout is not None
    last_cursor = since
    matched = False
    try:
        for line in proc.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                print(line, file=sys.stderr)
                continue
            if not isinstance(event, dict):
                continue
            cursor = event.get("cursor")
            if cursor is not None:
                last_cursor = str(cursor)
            if event_matches(event, rules):
                json.dump(event, sys.stdout, separators=(",", ":"))
                sys.stdout.write("\n")
                matched = True
                break
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
        if last_cursor:
            print(f"last_cursor={last_cursor}", file=sys.stderr)
    return 0 if matched else 1


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Wait for one pullfrog watch event, then exit."
    )
    parser.add_argument("--pr", type=int, required=True)
    parser.add_argument(
        "--until",
        required=True,
        help="preset (approval, ci, merged, review) or kind:value list",
    )
    parser.add_argument("repo", nargs="?", help="owner/repo if cwd remote is wrong")
    parser.add_argument("--since", help="resume from a prior event cursor")
    args = parser.parse_args()
    return run(args.pr, args.until, args.repo, args.since)


if __name__ == "__main__":
    raise SystemExit(main())
