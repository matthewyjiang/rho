---
name: pr-watch
description: >
  Babysit an open GitHub PR until review, CI, comments, or state changes
  need a reaction. Use when the user says watch a PR, babysit a PR, wait
  on review or checks, sit on a PR until it merges, or keep an eye on PR
  activity. Prefer this over polling `gh` or the GitHub API. Start the
  wait helper as a background process, then either do other work or stop.
  Do not inspect that process while it runs. When it exits, its stdout is
  the event.
compatibility: bun or npx, python3, `gh auth login`, Pullfrog GitHub App on the repo.
---

# Watch / babysit a PR

Snapshot, start the waiter as a background process, then do other work or
stop. When the process exits, stdout is one JSON event. That is the only
result. Do not inspect the process while it runs. Do not poll GitHub.

https://docs.pullfrog.com/watch.md

## Wait

```bash
gh pr view <number> --json number,title,state,isDraft,reviewDecision,statusCheckRollup,url,headRefName
gh pr checks <number>
python3 .agents/skills/pr-watch/wait.py --pr <number>
```

Run that last command in the background with a long timeout (900s). Leave
it alone until it exits.

Need `gh auth token` and the Pullfrog app on the repo or the stream is empty.
Resolve a missing PR number from `gh pr view --json number` or ask. Pass
`owner/repo` before `--pr` if the cwd remote is wrong. `bunx` then `npx`.

Default `--until` is `react`: any review, review comment, or PR comment;
check failure; PR closed or merged. Other presets: `approval`, `ci`,
`merged`, or `kind` / `kind:value,...`.

## After it returns

Read the event. Fetch `data.url` only if you will act. stderr prints
`last_cursor=` on every event. Exit 2 means `pullfrog watch` failed; fail
loud, do not poll GitHub instead.

| Event | Next step |
| --- | --- |
| review or comment | load it; fix real work, else wait again |
| `check` / `failure` | diagnose, fix, push, wait again |
| `review` / `approved` | stop if that was the goal; else wait for `ci` |
| `pr` closed or merged | stop and return the PR URL |

Wait again with `--since <cursor>`. No `--since` starts from now.
