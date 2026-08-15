---
name: pr-watch
description: >
  Babysit an open GitHub PR until review, CI, comments, or state changes
  need a reaction. Use when the user says watch a PR, babysit a PR, wait
  on review or checks, sit on a PR until it merges, or keep an eye on PR
  activity. Prefer this over polling `gh` or the GitHub API. Use
  `pullfrog watch` through the blocking wait helper, not a process poll loop.
compatibility: bun or npx, python3, `gh auth login`, Pullfrog GitHub App on the repo.
---

# Watch / babysit a PR

Block until the event you care about. Do not poll GitHub. Do not poll a
background `pullfrog watch` with the process tool. Rho's process poll waits
at most 30s, so that loop is just polling.

https://docs.pullfrog.com/watch.md

## Wait

Snapshot once, then run **one** foreground command that exits on the first
matching event. Give the shell a long `timeout_seconds` (900 is a good
default). The helper prints that event as one JSON line on stdout.

```bash
python3 .agents/skills/pr-watch/wait.py --pr <number> --until approval
```

| `--until` | Exits on |
| --- | --- |
| `approval` | review approved or changes_requested, PR closed/merged, check failure |
| `review` | review approved or changes_requested, PR closed/merged |
| `ci` | check success or failure, PR closed/merged |
| `merged` | PR closed or merged |
| `kind:value,...` | raw match, e.g. `review:approved,check:failure` |

Same helper with `owner/repo` if the cwd remote is wrong:

```bash
python3 .agents/skills/pr-watch/wait.py owner/repo --pr <number> --until approval
```

`bunx pullfrog watch` is the stream. Use `npx` only if `bunx` is missing.
Do not run the raw stream as a long-lived process you then peek at.

## Setup

Snapshot first; watch will not replay:

```bash
gh pr view <number> --json number,title,state,isDraft,reviewDecision,statusCheckRollup,url,headRefName
gh pr checks <number>
```

Need a working `gh auth token` and the Pullfrog app on the repo or the
stream stays empty. Resolve a missing PR number from `gh pr view --json number`
or ask.

## After an exit

Read the JSON event. Fetch full detail from `data.url` only when you will act.
stderr ends with `last_cursor=...`.

| Event | Next step |
| --- | --- |
| `review` / `changes_requested` | load comments, fix, push, wait again with `--since` |
| `review` / `approved` | stop if review was the goal; else `--until ci` |
| `check` / `failure` | diagnose, fix, push, wait again |
| `check` / `success` | report only if that was the wait |
| `pr` closed or merged | stop and return the PR URL |

To wait again after a timeout or a push:

```bash
python3 .agents/skills/pr-watch/wait.py --pr <number> --until approval --since <cursor>
```

No `--since` means start from now, no history.

## Do not

- Start `pullfrog watch` with the process tool and poll it every 30s.
- Loop on `gh pr view` / `gh pr checks`.
- Swallow auth, missing-app, or stream errors. Fail loud.

Auth, missing app, or stream errors: fail loud. Do not fall back to polling.
