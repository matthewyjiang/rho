# Edit format

Parent: [Tools and workspace](/tools-workspace).

`edit` applies one or more line-anchored hunks to existing UTF-8 files. Pass a hashline document in `input`. Take path tags and line numbers from a `read_file` result or from a prior successful `edit` response preview. Never invent a tag. `read_file` returns UTF-8 source files as a hashline view: a `[path#TAG]` header plus `N:line` rows. `TAG` is a 4-hex snapshot of the full file, computed with trailing whitespace ignored so a whitespace-only change does not invalidate a read. `offset`/`limit` select which rows are shown; the file is still read fully to mint `TAG`. `edit` rejects a stale `TAG` before writing.

```json
{
  "input": "[src/app.py#A1B2]\nPUT 2:\n+print(\"Hello, world!\")\n"
}
```

Supported ops:

- `PUT N:` replace one original line (digits then colon — never `PUT N.:`)
- `PUT N.=M:` replace inclusive original lines `N` through `M` with `+` body rows
- `PUT <N:` / `PUT >N:` / `PUT >$:` insert body rows before line N, after line N, or at end of file
- `CUT N.=M` or `CUT N` delete inclusive original lines (no colon on CUT)

Locators must match those forms exactly. A trailing dot (`PUT 12.:`, `PUT 12.=:`) is invalid and is rejected with an explicit error — it is not a single-line shorthand.

Rules:

- Take `TAG` and line numbers from the latest snapshot for that path: `read_file`, `grep` (content mode TAG + line numbers), a successful `edit` preview, a `write` chain snapshot, or a failed `edit` live snapshot. Grep match previews are not PUT bodies.
- Put every hunk for one path in a single `edit` document. Do not issue two `edit` tool calls on the same path in one batch; wait for the result first. Different paths may edit in parallel
- Line numbers name the original snapshot; they do not shift mid-document
- Every body row under a `:` header starts with `+` (use `+` alone for a blank line)
- `PUT` always needs at least one `+` body row; use `CUT` to delete
- Stale tags, overlapping destructive ranges, duplicate paths, out-of-range lines, and mid-edit file changes fail closed with no write. The error includes a bounded live snapshot - copy that header and lines to retry
- Re-read only for lines outside the live snapshot or post-edit preview
- After a large or structural edit, re-read before further ops on anchors outside the returned preview
- An insert whose anchor falls inside a range that another op replaces or deletes is rejected, because that position no longer exists after the edit
- Block ops (`N*`), registers, `REM`, and `MV` are not supported yet
- Create or fully rewrite files with `write`. Do not use `edit` to create paths

Successful `edit` results return a one-line ops summary (for example `PUT 2.=5: (4 → 2 line(s))`) plus a post-edit `[path#NEW]` numbered preview around the change for chaining. **Structural** edits (a single replace/delete span of 40+ original lines) return the new TAG and ops summary **without** numbered body lines so the next op must re-read. Successful `write` results return a bounded head/tail hashline snapshot with the new TAG. Unified diffs are tool metadata for UI cards, not repeated in model-facing content.

Streaming cards project the edit document alone (op summaries + PUT bodies). Approval and start cards dry-run against live files when readable so removals appear as real `-` rows; missing or stale targets fall back to the document projection.

Use `edit` when you have a fresh hashline snapshot and need one or more line-anchored hunks. Use `write` to create or fully rewrite a file. Do not use shell or Python to rewrite UTF-8 sources that `edit` can express.

## One read format for every caller

`read_file` returns the hashline view for every UTF-8 text file, whether or not the caller can use `edit`. This is deliberate. Two read formats would make the output depend on the agent's tool set, so the same file would read differently to a subagent, a workflow step, and the automation CLI, and any prompt or parser downstream would have to handle both. One format costs a small number of input tokens per line and keeps every reader on the same contract.
