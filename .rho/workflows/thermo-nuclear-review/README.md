# Thermo-nuclear review workflow

Runs three parallel read-only thermo-nuclear reviews on the current branch
change set, then applies the suggested fixes.

## Graph

1. `collect_context` - writes a git context pack to `target/thermo-nuclear-review/context.md`
2. Parallel review lanes (only if there are in-scope changes):
   - `structure_judo` - standards 0 and 3 (code judo / design cleaning)
   - `spaghetti_flow` - standards 1, 2, 4, and 7 (file size, spaghetti, magic, orchestration)
   - `boundaries_contracts` - standards 5 and 6 plus correctness, security, performance, and tests
3. `apply_fixes` - worker applies blocker/major findings from all three lanes
4. `no_changes` - cheap no-op path when the change set is empty

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
