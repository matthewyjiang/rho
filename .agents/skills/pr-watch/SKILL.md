---
name: pr-watch
description: >
  Babysit an open GitHub PR until review, CI, comments, or state changes
  need a reaction. Use when the user says watch a PR, babysit a PR, wait
  on review or checks, sit on a PR until it merges, or keep an eye on PR
  activity. Prefer this over polling `gh` or the GitHub API. Start the
  wait helper as a background process, then either do other work or end
  the turn. Never poll that process. The harness delivers the event when
  it exits.
compatibility: bun or npx, python3, `gh auth login`, Pullfrog GitHub App on the repo.
---

# Watch / babysit a PR

Start a waiter that exits on the event you care about. Then either work
on something else or end the turn. When the waiter exits, the harness
delivers the event. That delivery is the only way you learn the result.

Do not poll GitHub. Do not poll the waiter. Do not call process `poll` /
`status` on it. Do not hold the turn on a foreground watch.

https://docs.pullfrog.com/watch.md

## Wait

1. Snapshot once (below).
2. Start the helper as a **background process** with a long timeout (900s
   is a good default):

```bash
python3 .agents/skills/pr-watch/wait.py --pr <number> --until approval
```

3. After `start` returns an id, you may do unrelated work, or you must
   end the turn. You may not wait on that id.
4. When the process exits, you get its stdout: one JSON event. Act then.

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
The helper wraps it and exits on the first match so you are not babysitting
the raw stream.

## Setup

Snapshot first; watch will not replay:

```bash
gh pr view <number> --json number,title,state,isDraft,reviewDecision,statusCheckRollup,url,headRefName
gh pr checks <number>
```

Need a working `gh auth token` and the Pullfrog app on the repo or the
stream stays empty. Resolve a missing PR number from `gh pr view --json number`
or ask.

## After the waiter returns

This section applies only after the harness delivers the process result.
Do not fetch or check in the meantime.

Read the JSON event. Fetch full detail from `data.url` only when you will act.
stderr ends with `last_cursor=...`.

| Event | Next step |
| --- | --- |
| `review` / `changes_requested` | load comments, fix, push, start another waiter with `--since` |
| `review` / `approved` | stop if review was the goal; else start `--until ci` |
| `check` / `failure` | diagnose, fix, push, start another waiter |
| `check` / `success` | report only if that was the wait |
| `pr` closed or merged | stop and return the PR URL |

To wait again after a timeout or a push, start another background waiter
and again either do other work or end the turn:

```bash
python3 .agents/skills/pr-watch/wait.py --pr <number> --until approval --since <cursor>
```

No `--since` means start from now, no history.

## Do not

- Run the waiter in the foreground and sit on the turn.
- `poll`, `status`, or otherwise wait on the background process.
- Loop on `gh pr view` / `gh pr checks` while the waiter is running.
- Swallow auth, missing-app, or stream errors. Fail loud.

Auth, missing app, or stream errors: fail loud. Do not fall back to polling.
