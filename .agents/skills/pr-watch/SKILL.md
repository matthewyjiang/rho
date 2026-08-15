---
name: pr-watch
description: >
  Babysit an open GitHub PR until review, CI, comments, or state changes
  need a reaction. Use when the user says watch a PR, babysit a PR, wait
  on review or checks, sit on a PR until it merges, or keep an eye on PR
  activity. Prefer this over polling `gh` or the GitHub API. Use
  `pullfrog watch`.
compatibility: bun or npx, `gh auth login`, Pullfrog GitHub App on the repo.
---

# Watch / babysit a PR

Stream with Pullfrog. Do not poll GitHub.

https://docs.pullfrog.com/watch.md

## Command

```bash
bunx pullfrog watch --pr <number>
bunx pullfrog watch owner/repo --pr <number>   # if cwd remote is wrong
```

Same args with `npx pullfrog` only if `bunx` is missing. Keep JSON lines;
`--pretty` is for humans. `--pr` is required. No `--since` means start from
now, no history.

## Setup

Snapshot first; watch will not replay:

```bash
gh pr view <number> --json number,title,state,isDraft,reviewDecision,statusCheckRollup,url,headRefName
gh pr checks <number>
```

Need a working `gh auth token` and the Pullfrog app on the repo or the
stream stays empty. Resolve a missing PR number from `gh pr view --json number`
or ask.

## Stream

Long-lived process. Keep the last `cursor`. Restart with `--since <cursor>`.

Each stdout line is one event: `kind`, `pr`, `createdAt`, `cursor`, `data`.
`data` is a teaser (actor, action, state, truncated body, url). Fetch full
detail only when you will act.

| Kind | Act when |
| --- | --- |
| `pr` | merged/closed: stop. `synchronize`: wait for `check`. |
| `review` | `changes_requested`: load, fix, push. `approved`: keep watching CI unless review was the only goal. |
| `review_comment` | new comments that are not yours and not noise. |
| `review_thread` | unresolved = open work. Do not reopen without cause. |
| `comment` | directed at this agent or asking for changes. Skip bot chatter unless it is a real failure. |
| `check` | failure: diagnose and fix. success: report only if that was the wait. |

Leave the watcher up while you work. After a push, wait for the next
`check`/`review` event. Stop on the user's exit condition (merged,
approved+green, review done, or they say stop) and return the PR URL.

Auth, missing app, or stream errors: fail loud. Do not fall back to polling.
