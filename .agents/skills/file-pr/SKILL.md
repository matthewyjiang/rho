---
name: file-pr
description: >
  Open, file, create, update, or submit a GitHub PR from the current branch.
  Triggers on "pr this", "open a pr", "file a pr", "make a pr", "ship this",
  or asking for a PR title/body. Prefer over ad-hoc gh pr create.
---

# File a pull request

Open a **real** PR (not a draft). Drafts skip review-bot coverage. Rebase onto
latest base first so reviewers do not fight stale conflicts.

Leave general ship prose, Conventional Commits, and template section rules to
AGENTS.md and the repo. This skill only covers the file flow and the title/body
shape that usually goes wrong.

## Workflow

1. **Orient** - status, commits ahead of base, diff against base. Note the base
   branch (upstream, else repo default). If this branch is on a `gh stack`, use
   the **gh-stack** skill instead of bare `gh pr create`.
2. **Prep** - fetch and rebase onto latest base. Push (`--force-with-lease` after
   rebase when the remote branch already exists). Do not surprise-commit dirt
   the user did not mean to ship.
3. **Write** title and body (below), then create or update immediately:
   - `gh pr create --title "..." --body-file <tmp.md> --base <base>`
   - or `gh pr edit --title "..." --body-file <tmp.md>` when a PR already exists
   - always `--body-file` (temp or heredoc file); never a multiline shell-quoted
     `--body` string (literal `\n` on GitHub)
   - no `--draft`
4. **Return** the PR URL.

## Title

Titles often become squash subjects. Match how recent merged PRs in this repo
read. Short, scannable, **why it matters** - not only the mechanism.

```text
BAD:  perf(server): negotiate permessage-deflate on the websocket
GOOD: perf(server): cut websocket frame size by 70%+ with gzipping

BAD:  fix: update thread code
GOOD: fix(web): new threads no longer spike CPU
```

## Body

Problem first (from the user's goal or the bug), then a brief solution. Do not
lead with an implementation inventory; reviewers have the diff.

```text
BAD:
Removed implicit workspace carry-over from every "new thread" entry point
(cmd+n / cmd+shift+o, sidebar v1/v2 buttons, command palette). New threads
inherit only the project from context; branch, worktree, and env mode always
come from the configured defaults. Deleted buildContextualThreadOptions,
startNewThreadInProjectFromContext, and the v1 sidebar's seed-context machinery.

GOOD:
My "new worktree" default was ignored when starting new threads on existing
worktrees. Super unintuitive. Now your preferences always apply.
```

Fill the repo PR template when present. End with one factual line naming the
**model** and **harness** that made the change. If multiple models were used, name them all and what parts they contributed to.
