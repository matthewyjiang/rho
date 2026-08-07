#!/usr/bin/env python3
"""Collect branch review context for the thermo-nuclear review workflow.

Writes a markdown context pack under target/ and prints a small JSON summary
on stdout for downstream workflow nodes.
"""

from __future__ import annotations

import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


def run(argv: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def require_git() -> None:
    result = run(["git", "rev-parse", "--is-inside-work-tree"])
    if result.returncode != 0 or result.stdout.strip() != "true":
        raise SystemExit("collect_context: not inside a git work tree")


def merge_base(candidate: str) -> str | None:
    probe = run(["git", "rev-parse", "--verify", f"{candidate}^{{commit}}"])
    if probe.returncode != 0:
        return None
    result = run(["git", "merge-base", "HEAD", candidate])
    if result.returncode != 0:
        return None
    return result.stdout.strip() or None


def resolve_base(requested: str) -> tuple[str, str]:
    """Return (label, commit) for the review base."""
    if requested:
        commit = merge_base(requested)
        if commit is None:
            raise SystemExit(
                f"collect_context: could not resolve requested base ref {requested!r}"
            )
        return requested, commit

    candidates = ["origin/main", "main", "origin/master", "master"]
    for candidate in candidates:
        commit = merge_base(candidate)
        if commit is not None:
            return candidate, commit

    raise SystemExit(
        "collect_context: could not discover a default review base; "
        f"tried {', '.join(candidates)}"
    )


def git_output(argv: list[str]) -> str:
    result = run(["git", *argv])
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip()
        raise SystemExit(
            f"collect_context: git {' '.join(argv)} failed: {detail}"
        )
    return result.stdout

@dataclass(frozen=True)
class NameStatus:
    status: str
    path: str
    source: str | None = None


def parse_name_status(output: str) -> list[NameStatus]:
    """Parse `git diff --name-status -z`, retaining rename/copy sources."""
    fields = output.split("\0")
    if fields and fields[-1] == "":
        fields.pop()

    entries: list[NameStatus] = []
    cursor = 0
    while cursor < len(fields):
        status = fields[cursor]
        cursor += 1
        path_count = 2 if status[:1] in {"R", "C"} else 1
        if not status or cursor + path_count > len(fields):
            raise ValueError("malformed git name-status output")
        if path_count == 2:
            source, path = fields[cursor : cursor + 2]
            cursor += 2
            entries.append(NameStatus(status=status, path=path, source=source))
        else:
            path = fields[cursor]
            cursor += 1
            entries.append(NameStatus(status=status, path=path))
    return entries


def format_name_status(entries: list[NameStatus]) -> str:
    lines = []
    for entry in entries:
        fields = [entry.status]
        if entry.source is not None:
            fields.append(entry.source)
        fields.append(entry.path)
        lines.append("\t".join(fields))
    return "\n".join(lines)


def parse_nul_paths(output: str) -> list[str]:
    return [path for path in output.split("\0") if path]


def deduplicate(paths: list[str]) -> list[str]:
    return list(dict.fromkeys(paths))


def main() -> int:
    require_git()

    base_input = sys.argv[1] if len(sys.argv) > 1 else "main"
    scope = sys.argv[2] if len(sys.argv) > 2 else "all"
    if scope not in {"all", "committed", "uncommitted"}:
        raise SystemExit(
            "collect_context: scope must be all, committed, or uncommitted"
        )

    root = Path(git_output(["rev-parse", "--show-toplevel"]).strip())
    head = git_output(["rev-parse", "HEAD"]).strip()
    branch = git_output(["branch", "--show-current"]).strip() or "DETACHED"
    base_label, base_commit = resolve_base(base_input)

    status = git_output(["status", "--short"])
    committed_entries = parse_name_status(
        git_output(
            ["diff", "--name-status", "--find-renames", "-z", f"{base_commit}...HEAD"]
        )
    )
    uncommitted_entries = parse_name_status(
        git_output(["diff", "--name-status", "--find-renames", "-z", "HEAD"])
    )
    untracked_paths = parse_nul_paths(
        git_output(["ls-files", "--others", "--exclude-standard", "-z"])
    )
    name_status_committed = format_name_status(committed_entries)
    name_status_uncommitted = format_name_status(uncommitted_entries)
    committed_paths = [entry.path for entry in committed_entries]
    uncommitted_paths = [entry.path for entry in uncommitted_entries]

    if scope == "committed":
        changed_names = deduplicate(committed_paths)
        diff_committed = git_output(["diff", "--find-renames", f"{base_commit}...HEAD"])
        diff_uncommitted = ""
        untracked_list: list[str] = []
    elif scope == "uncommitted":
        changed_names = deduplicate(uncommitted_paths + untracked_paths)
        diff_committed = ""
        diff_uncommitted = git_output(["diff", "--find-renames", "HEAD"])
        untracked_list = untracked_paths
    else:
        changed_names = deduplicate(committed_paths + uncommitted_paths + untracked_paths)
        diff_committed = git_output(["diff", "--find-renames", f"{base_commit}...HEAD"])
        diff_uncommitted = git_output(["diff", "--find-renames", "HEAD"])
        untracked_list = untracked_paths

    shortstat_committed = git_output(
        ["diff", "--shortstat", f"{base_commit}...HEAD"]
    ).strip()
    shortstat_uncommitted = git_output(["diff", "--shortstat", "HEAD"]).strip()

    out_dir = root / "target" / "thermo-nuclear-review"
    out_dir.mkdir(parents=True, exist_ok=True)
    context_path = out_dir / "context.md"

    # Cap huge diffs so read-only reviewers stay within practical context.
    max_diff_chars = 350_000

    def clip(label: str, text: str) -> str:
        if len(text) <= max_diff_chars:
            return text
        omitted = len(text) - max_diff_chars
        return (
            text[:max_diff_chars]
            + f"\n\n... [{label} truncated; {omitted} more chars omitted] ...\n"
        )

    lines = [
        "# Thermo-nuclear review context",
        "",
        f"- branch: `{branch}`",
        f"- head: `{head}`",
        f"- base_label: `{base_label}`",
        f"- base_commit: `{base_commit}`",
        f"- scope: `{scope}`",
        f"- committed_shortstat: {shortstat_committed or '(none)'}",
        f"- uncommitted_shortstat: {shortstat_uncommitted or '(none)'}",
        "",
        "## Changed files",
        "",
    ]
    if changed_names:
        lines.extend(f"- `{name}`" for name in changed_names)
    else:
        lines.append("- (none)")

    lines.extend(["", "## git status --short", "", "```", status.rstrip(), "```", ""])

    if name_status_committed.strip() and scope != "uncommitted":
        lines.extend(
            [
                "## committed name-status",
                "",
                "```",
                name_status_committed.rstrip(),
                "```",
                "",
            ]
        )
    if (name_status_uncommitted.strip() or untracked_list) and scope != "committed":
        lines.extend(
            [
                "## uncommitted name-status",
                "",
                "```",
                name_status_uncommitted.rstrip(),
                "```",
                "",
            ]
        )
        if untracked_list:
            lines.extend(
                [
                    "## untracked files",
                    "",
                    "```",
                    "\n".join(untracked_list),
                    "```",
                    "",
                ]
            )

    if diff_committed and scope != "uncommitted":
        lines.extend(
            [
                f"## committed diff ({base_label}...HEAD)",
                "",
                "```diff",
                clip("committed diff", diff_committed).rstrip(),
                "```",
                "",
            ]
        )
    if diff_uncommitted and scope != "committed":
        lines.extend(
            [
                "## uncommitted diff (HEAD vs worktree)",
                "",
                "```diff",
                clip("uncommitted diff", diff_uncommitted).rstrip(),
                "```",
                "",
            ]
        )
    if untracked_list and scope != "committed":
        lines.extend(
            [
                "## untracked file note",
                "",
                "Untracked paths are listed above. Open them with read tools when relevant.",
                "",
            ]
        )

    context_path.write_text("\n".join(lines) + "\n", encoding="utf-8")

    summary = {
        "branch": branch,
        "head": head,
        "base_label": base_label,
        "base_commit": base_commit,
        "scope": scope,
        "context_path": str(context_path.relative_to(root)),
        "changed_files": changed_names,
        "changed_file_count": len(changed_names),
        "committed_shortstat": shortstat_committed,
        "uncommitted_shortstat": shortstat_uncommitted,
        "has_changes": bool(changed_names),
    }
    sys.stdout.write(json.dumps(summary, ensure_ascii=False, separators=(",", ":")))
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
