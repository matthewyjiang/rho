# Thermo-nuclear review workflow

Runs three parallel read-only thermo-nuclear reviews on the current branch
change set, then applies the suggested fixes locally. Set `ship=true` to opt
in to publishing reviewed work and babysitting the PR until it is merge ready.

## Graph

1. `collect_context` - writes a git context pack to `target/thermo-nuclear-review/context.md`
2. Parallel review lanes (only if there are in-scope changes):
   - `structure_judo` - standards 0 and 3 (code judo / design cleaning)
   - `spaghetti_flow` - standards 1, 2, 4, and 7 (file size, spaghetti, magic, orchestration)
   - `boundaries_contracts` - standards 5 and 6 plus correctness, security, performance, and tests
3. `apply_fixes` - worker applies blocker/major findings from all three lanes
4. `no_changes` - cheap no-op path when the change set is empty
5. Only with `ship=true`, `ship_and_babysit` runs after `apply_fixes`. It
   validates, commits, and pushes verified fix-stage edits, opens the PR if
   needed, then follows `pr-watch` until approval, green required checks, and
   no unresolved review thread. It never merges. Unresolved blockers or
   unsafe changes are reported as `blocked`. The empty-scope path stays a
   cheap terminal no-op, even with shipping enabled.

Shipping needs `gh auth login`, `bunx` or `npx`, `timeout`, and the Pullfrog
GitHub App on the repository (see `pr-watch`). An opt-in `ship_preflight` runs
before context collection and review agents to check local prerequisites and
reject any pre-existing staged, unstaged, or untracked WIP. Commit WIP first
or use the default local-only mode; the shepherd never tries to split mixed
user/fix hunks. The clean review-start HEAD is its baseline snapshot. The
Pullfrog App must be installed separately; a failed watch stream is reported
as `blocked`. The shepherd's node timeout is 6 hours.

The unattended shepherd may act on review requests only from repository
`OWNER`, `MEMBER`, or `COLLABORATOR` authors, or verified Pullfrog/CodeRabbit
App bots. Other requests are reported in `open_items`, not applied as edits.

## Result

The run exports the reviewed change set: `branch`, `head`, `base_commit`,
`changed_files`, and `has_changes`. `rho workflow status` prints them as the
root scope result. The fix summary stays in the `apply_fixes` or `no_changes`
node output, and the PR outcome (`merge_ready`, `merged`, `closed`, `no_pr`,
or `blocked`, with the PR URL and structured SHA/subject commit records)
stays in the optional `ship_and_babysit` node output. These nodes do not run
on every path, and every export must resolve for a successful run.

Planning `exports` needs a Rho release with scoped workflow programs; 2.13 and
earlier reject the field.

## Inputs

| Input | Default | Meaning |
| --- | --- | --- |
| `base` | `main` | Git ref used for merge-base comparison |
| `scope` | `all` | `all`, `committed`, or `uncommitted` |
| `focus_path` | `.` | Optional narrowing hint for reviewers |
| `ship` | `false` | Opt in to commit, push, PR creation, and unattended review-thread handling; requires a clean tree |

## Usage

```bash
rho workflow validate .rho/workflows/thermo-nuclear-review/workflow.star

rho workflow plan .rho/workflows/thermo-nuclear-review/workflow.star \
  --input 'base="main"' \
  --input 'scope="all"'

rho workflow run <PLAN_ID> --yes
rho workflow status <RUN_ID>
```

For publishing and babysitting, add `--input 'ship=true'` to the plan
command. Without it, the workflow does not commit, push, or touch GitHub.

Or open `/workflow` in the TUI and start `thermo-nuclear-review`.

## Test the context collector

```bash
python3 .rho/workflows/thermo-nuclear-review/test_collect_context.py
```
