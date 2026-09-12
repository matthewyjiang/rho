---
name: file-pr
description: >
  Open, file, create, update, or submit a GitHub PR from the current branch.
  Triggers on "pr this", "open a pr", "file a pr", "make a pr", "ship this",
  or asking for a PR title/body. Prefer over ad-hoc gh pr create.
---

# File a pull request

## Choose the requested action

- A request for a PR title/body means produce text only. Do not commit, rebase,
  push, or create/edit a PR on GitHub.
- An explicit request to edit a PR's title/body authorizes that metadata update,
  not a branch rewrite or push. Inspect the PR, write the requested text, update
  with `gh pr edit --body-file <tmp.md>` and/or `--title`, and return the URL.
- An explicit request to open, file, submit, or ship a PR uses the publishing
  workflow below. For updates, publish code only when requested.

Loading this skill does not authorize publishing. Ask if the requested action
is ambiguous. New PRs default to ready for review, not drafts, so review bots
run; honor an explicit request for a draft.

Leave general ship prose, Conventional Commits, and template section rules to
AGENTS.md and the repo. This skill only covers the file flow and the title/body
shape that usually goes wrong.

## Publishing workflow

1. **Orient** - status, commits ahead of base, diff against base. Note the base
   branch from the existing PR or stack, else the repository default; a remote
   tracking branch is not necessarily the PR base. If this branch is on a
   `gh stack`, use the **gh-stack** skill instead of bare `gh pr create`.
2. **Prep** - fetch and inspect divergence from the base and remote branch.
   Rebase when needed and safe on a task-owned branch with a clean working tree.
   Ask before rewriting a shared branch or when ownership is unclear. Do not
   overwrite remote work or surprise-commit unrelated changes. Use stack-aware
   operations for stacked branches, not a bare rebase onto the repository default.
3. **Validate and push** - follow `rho-rust-change-validation` for check selection
   and reuse of current results. Push only the requested work. After an authorized
   rebase, use `--force-with-lease` if the remote branch already exists; never
   force-push over unexpected remote changes. For stacks, default to
   `gh stack submit --auto --open` so new PRs are ready for review; omit `--open`
   only when the user requests drafts. Confirm the affected layers before submission.
4. **Write** title and body (below), then create or update:
   - `gh pr create --title "..." --body-file <tmp.md> --base <base>`
   - or `gh pr edit --title "..." --body-file <tmp.md>` when a PR already exists
   - always `--body-file` (temp or heredoc file); never a multiline shell-quoted
     `--body` string (literal `\n` on GitHub)
   - use `--draft` only when explicitly requested
5. **Return** the PR URL.

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
