# Hashline edit eval design

Design-only. Do not treat this as a shipped runner. Use it to compare
`hashline_edit`, `edit_file`, and `apply_patch` under Rho's real tool loop.

## Goal

Measure whether line-anchored hashline edits raise task success and cut retry
waste versus the current dual stack, without training a model.

Primary questions:

1. Does hashline beat `edit_file` on multi-hunk single-file work?
2. Does hashline beat `apply_patch` for non-Codex models?
3. Does Codex still need `apply_patch` for multi-file add/delete work?
4. What is the input-token tax of hashline `read_file` views?
5. Do stale-tag failures recover cleanly with one re-read?

## Non-goals

- Live model leaderboard claims in CI
- Full oh-my-pi suite clone as a gate
- Training or fine-tuning an apply model

## Fixture families

Build small, deterministic workspaces. Prefer synthetic files over large vendor
trees so diffs stay reviewable.

| Family | Setup | Success rule |
| --- | --- | --- |
| A. Mechanical revert | Mutate a source file with one known inverse fix (operator swap, off-by-one, renamed local, deleted guard) | Formatted file equals pre-mutation gold |
| B. Multi-hunk single file | 2-4 disjoint edits in one file from one prompt | Exact gold or AST-equivalent gold |
| C. Multi-file coordinated | Two or three existing files must change together | All targets match gold; no extra files |
| D. Partial-read edit | File is longer than a typical read window; bug sits outside the first chunk | Gold match after ranged reads |
| E. Grep-first edit | Prompt names a symbol; agent should grep then edit | Gold match; record whether a full read happened |
| F. Stale snapshot | After the agent reads, a harness hook rewrites the file before the edit tool runs | Edit must fail closed or re-read and succeed; never silent wrong apply |
| G. Formatter churn | After read, rewrite whitespace-only (trailing spaces, EOL) while keeping semantics | Tag policy match: normalized hash accepts intentional normalize; raw drift fails closed as designed |
| H. Create / delete | Need new file or delete path | `write_file` / `apply_patch` path; hashline alone must not invent creates |
| I. Ambiguous string replace | Same token appears many times; only one site is correct | Gold match; useful contrast for `edit_file` uniqueness failures |

Start with 30 tasks per family A-E, 10 for F-I. Keep a frozen manifest with
content hashes so reruns stay comparable.

## Tool treatments

Run the same prompts under fixed tool sets:

| Treatment | Tools |
| --- | --- |
| `replace` | `read_file`, `edit_file`, `write_file`, `grep`, `glob`, `list_dir` |
| `patch` | `read_file`, `apply_patch`, `write_file`, `grep`, `glob`, `list_dir` |
| `hashline` | `read_file`, `hashline_edit`, `write_file`, `grep`, `glob`, `list_dir` |
| `rho-default` | full coding set including all three edit tools |

Optional later:

| Treatment | Tools |
| --- | --- |
| `hashline+patch` | hashline plus `apply_patch` for create/delete only guidance |

Keep shell off for the core bake-off so models cannot escape the edit contract
with `sed`.

## Agent loop

- Fresh Rho session per task
- Same system/tool prompts as product defaults for that treatment
- Cap turns (suggested start: 12) and wall time (suggested start: 3 minutes)
- Capture full tool traces, token usage, and final workspace tree
- Format gold and candidate with the same formatter before compare

Do not share conversation state across tasks.

## Metrics

Report medians with bootstrap intervals when n allows.

| Metric | Definition |
| --- | --- |
| Pass@1 | Fraction of tasks whose final tree matches gold after format |
| Edit success rate | Fraction of edit-tool calls that return ok |
| Mechanical fail rate | Edit failures whose error is format/match/tag/parse, not policy deny |
| Retry loops | Mean edit failures before first successful edit per task |
| Turns to solve | Mean model turns on passes |
| Input tokens | Mean prompt+tool input tokens |
| Output tokens | Mean completion tokens |
| Read amplification | Mean `read_file` calls per task |
| Time to solve | Wall time on passes |
| Wrong-apply rate | Failed gold with at least one successful edit (silent corruption) |

Slice metrics by model and by fixture family.

## Models

Run at least:

- one strong Anthropic model
- one OpenAI Codex-oriented model
- one mid open or fast model you care about for multi-provider users

Same temperature and reasoning settings across treatments.

## Harness sketch

Not implemented in this PR. Suggested layout later:

```text
crates/rho-edit-bench/
  README.md
  fixtures/
    manifest.json
    tasks/<id>/
      prompt.md
      workspace/          # starting tree
      gold/               # expected tree
      meta.json           # family, mutation notes
  src/
    main.rs               # run one treatment x model x task
    score.rs              # format + tree compare
    report.rs             # markdown/json summary
```

Runner responsibilities:

1. Copy `workspace/` to a temp dir
2. Launch Rho automation with the treatment tool allowlist
3. Pass `prompt.md` as the user message
4. Stop on idle, turn cap, or timeout
5. Format and diff against `gold/`
6. Emit one JSON record per run

Unit tests already own parser/apply correctness. This bench owns
model-in-the-loop expression quality only.

## Pass bars for product decisions

Use these as decision aids, not automatic merge gates.

| Decision | Evidence needed |
| --- | --- |
| Keep hashline as default multi-hunk path | `hashline` pass@1 >= `replace` on B/D and >= `patch` on non-Codex models for A-C |
| Keep `apply_patch` | Codex treatment still wins H or multi-file add/delete by a clear margin |
| Drop promoting hashline | Wrong-apply rate rises, or input-token tax erases output-token savings with no pass gain on strong models |
| Teach tool-choice heuristics | In `rho-default`, share of successful edits by tool and mis-routing rate |

## Smoke set (manual, pre-runner)

Before building the full runner, walk five tasks by hand:

1. Single-line replace after full read
2. Three-hunk edit in one file
3. Two-file edit in one hashline document
4. Stale tag after external rewrite
5. Create new file (must use `write_file`)

Record pass/fail, tool calls, and whether error text was enough to recover.

## Relation to external work

Inspired by Can Bölük's hashline harness write-up and react-edit style
mechanical bugs. Rho's product loop adds grep, partial reads, multi-file
sections, rewind checkpoints, and tool-choice among three editors. Those are
first-class here; do not score only isolated edit-format puzzles.
