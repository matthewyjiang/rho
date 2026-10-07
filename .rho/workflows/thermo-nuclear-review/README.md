# Thermo-nuclear review workflow

Runs three parallel read-only thermo-nuclear reviews on the current branch
change set, applies the suggested fixes, then ships them and babysits the PR
until it is merge ready.

## Graph

0. `ship_preflight` - fails fast when `gh` auth, `bunx`/`npx`, or `timeout`
   is missing, before any review agent runs
1. `collect_context` - writes a git context pack to `target/thermo-nuclear-review/context.md`.
   It opens with an Intent section: the branch PR's title and body (when `gh`
   finds one) and the commit messages since the base. These author-controlled
   values are HTML-escaped JSON strings inside labeled tags, not instructions.
2. Parallel review lanes (only if there are in-scope changes):
   - `structure_judo` - standards 0 and 3 (code judo / design cleaning)
   - `spaghetti_flow` - standards 1, 2, 4, and 7 (file size, spaghetti, magic, orchestration)
   - `boundaries_contracts` - standards 5 and 6 plus correctness, security, performance, and tests
3. `apply_fixes` - worker applies blocker/major findings from all three lanes.
   The change's intent comes first: a finding that can only be fixed by
   removing or gating the behavior the change adds is skipped as
   `conflicts with intent`, and the stage reports `partial` instead of
   `fixed` (or `blocked` when nothing safe can be applied). The shepherd treats
   every intent-conflict skip as a hard stop: it reports `blocked` and lists the
   conflict in `open_items`, even if an existing PR is approved with green CI.
4. `no_changes` - cheap no-op path when the change set is empty
5. Babysit the PR with the `pr-shepherd` agent, on whichever path ran:
   - `ship_and_babysit` (after `apply_fixes`, including `partial` and
     `blocked`) - validates, commits, and pushes the reviewed change set first
   - `babysit_unchanged` (after `no_changes`) - the branch may still have an
     open PR or commits outside the reviewed scope

   The shepherd opens the PR if the branch has none, then follows the
   `pr-watch` skill until the PR is approved with every required check green
   and no open review thread. It never merges. With nothing to ship and no
   PR, it reports `no_pr`. The two nodes share one definition; they are
   separate only because a template cannot read a skipped node's output.

The shepherd pushes code and talks to GitHub, so it also needs the Pullfrog
GitHub App on the repository (see the `pr-watch` skill). It only acts on
review requests from verified repository owners, members, or collaborators,
or from the Pullfrog and CodeRabbit apps; everything else goes to its
`open_items`. Its node timeout is 6 hours.

## Result

The run exports the reviewed change set: `branch`, `head`, `base_commit`,
`changed_files`, and `has_changes`. `rho workflow status` prints them as the
root scope result. The fix summary stays in the `apply_fixes` or `no_changes`
node output, and the PR outcome (`merge_ready`, `merged`, `closed`, `no_pr`,
or `blocked`, with the PR URL and `{sha, subject}` commit records) stays in
the `ship_and_babysit` or `babysit_unchanged` node output, because those
nodes do not run on every path and every export must resolve for a
successful run.

Planning `exports` needs a Rho release with scoped workflow programs; 2.13 and
earlier reject the field.

## Inputs

| Input | Default | Meaning |
| --- | --- | --- |
| `base` | `main` | Git ref used for merge-base comparison |
| `scope` | `all` | `all`, `committed`, or `uncommitted` |
| `focus_path` | `.` | Optional narrowing hint for reviewers |

## Usage

```bash
rho workflow validate .rho/workflows/thermo-nuclear-review/workflow.star

rho workflow plan .rho/workflows/thermo-nuclear-review/workflow.star \
  --input 'base="main"' \
  --input 'scope="all"'

rho workflow run <PLAN_ID> --yes
rho workflow status <RUN_ID>
```

Or open `/workflow` in the TUI and start `thermo-nuclear-review`.

## Test the context collector

```bash
python3 .rho/workflows/thermo-nuclear-review/test_collect_context.py
```
