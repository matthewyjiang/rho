#!/usr/bin/env python3
"""Block until a pullfrog watch event matches, then exit.

Prints the matching JSON event on stdout. Prints last_cursor= on stderr
as each event arrives so a timeout kill still leaves a resume point.
"""

from __future__ import annotations

import argparse
import json
import shutil
import signal
import subprocess
import sys
from typing import Any

# (kind, field, value). field is None: any event of that kind.
Rule = tuple[str, str | None, str | None]

PRESETS: dict[str, tuple[Rule, ...]] = {
    # Default babysit: wake on work, not only the final stamp.
    "react": (
        ("review", None, None),
        ("review_comment", None, None),
        ("comment", None, None),
        ("check", "conclusion", "failure"),
        ("pr", "action", "closed"),
        ("pr", "action", "merged"),
    ),
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
}

KIND_FIELDS = {
    "review": "state",
    "check": "conclusion",
    "pr": "action",
    "review_thread": "action",
    "review_comment": "action",
    "comment": "action",
}

def parse_until(raw: str) -> tuple[Rule, ...]:
    if raw in PRESETS:
        return PRESETS[raw]
    rules: list[Rule] = []
    for part in raw.split(","):
        part = part.strip()
        if not part:
            continue
        if part in PRESETS:
            rules.extend(PRESETS[part])
            continue
        kind, sep, value = part.partition(":")
        if not kind:
            raise SystemExit(
                f"bad --until item {part!r}: use a preset ({', '.join(PRESETS)}) "
                "or kind / kind:value"
            )
        if not sep:
            rules.append((kind, None, None))
            continue
        field = KIND_FIELDS.get(kind, "action")
        rules.append((kind, field, value.lower()))
    if not rules:
        raise SystemExit("empty --until")
    return tuple(rules)


def event_actor(event: dict[str, Any]) -> str:
    data = event.get("data")
    if not isinstance(data, dict):
        data = {}
    for key in ("actor", "author", "reviewer"):
        value = data.get(key) or event.get(key)
        if value:
            return str(value).lower()
    return ""


def is_react_noise(event: dict[str, Any]) -> bool:
    if str(event.get("kind") or "") not in {"review", "review_comment", "comment"}:
        return False
    actor = event_actor(event)
    if not actor or "pullfrog" in actor or "coderabbit" in actor:
        return False
    return "[bot]" in actor


def event_matches(event: dict[str, Any], rules: tuple[Rule, ...]) -> bool:
    kind = str(event.get("kind") or "")
    data = event.get("data")
    if not isinstance(data, dict):
        data = {}
    for rule_kind, field, value in rules:
        if kind != rule_kind:
            continue
        if field is None:
            return True
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


def _note_cursor(cursor: str) -> None:
    print(f"last_cursor={cursor}", file=sys.stderr, flush=True)


def run(pr: int, until: str, repo: str | None, since: str | None) -> int:
    rules = parse_until(until)
    skip_noise = "react" in {part.strip() for part in until.split(",")}
    cmd = watch_command(pr, repo, since)
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        text=True,
    )
    assert proc.stdout is not None
    last_cursor = since
    matched = False

    def on_term(_signum: int, _frame: Any) -> None:
        if last_cursor:
            _note_cursor(last_cursor)
        raise SystemExit(143)

    signal.signal(signal.SIGTERM, on_term)
    try:
        for line in proc.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                print(line, file=sys.stderr, flush=True)
                continue
            if not isinstance(event, dict):
                continue
            cursor = event.get("cursor")
            if cursor is not None:
                last_cursor = str(cursor)
                _note_cursor(last_cursor)
            if event_matches(event, rules):
                if skip_noise and is_react_noise(event):
                    continue
                json.dump(event, sys.stdout, separators=(",", ":"))
                sys.stdout.write("\n")
                sys.stdout.flush()
                matched = True
                break
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
    if matched:
        return 0
    code = proc.returncode
    if code not in (0, None, -signal.SIGTERM):
        print(f"pullfrog watch failed: exit {code}", file=sys.stderr)
        return 2
    return 1


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Wait for one pullfrog watch event, then exit."
    )
    parser.add_argument("--pr", type=int, required=True)
    parser.add_argument(
        "--until",
        default="react",
        help="preset (react, approval, ci, merged) or kind / kind:value list",
    )
    parser.add_argument("repo", nargs="?", help="owner/repo if cwd remote is wrong")
    parser.add_argument("--since", help="resume from a prior event cursor")
    args = parser.parse_args()
    return run(args.pr, args.until, args.repo, args.since)


if __name__ == "__main__":
    raise SystemExit(main())
