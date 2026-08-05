# Hashline edit eval

Historical comparison notes for Rho's line-anchored `edit` tool under the real
tool loop. `edit_file` and `apply_patch` were removed from the product; `edit`
is the only multi-hunk workspace edit tool. Prefer `write_file` to create or
fully rewrite files.

## Questions this eval answers

1. Does hashline `edit` beat free-form rewrite on multi-hunk single-file work?
2. How often do stale tags force a re-read?
3. What is the token cost of always returning hashline views from `read_file`?
4. When should the agent choose `write_file` instead of `edit`?

## Suites

| Suite | Intent | Gold / notes |
| --- | --- | --- |
| A. Single-hunk | One line change from a fresh read | Exact file match |
| B. Multi-hunk same file | Several non-overlapping edits | Exact file match |
| C. Multi-file | One document, several paths | Exact file match |
| D. Stale tag | Concurrent change invalidates TAG | Must fail closed then re-read or copy prior edit preview |
| E. Out of range | Line numbers past EOF | Must fail closed |
| F. Insert anchors | Before/after/end inserts | Exact file match |
| G. Delete ranges | CUT inclusive ranges | Exact file match |
| H. Create / delete path | Need new file or remove path | `write_file` / shell; `edit` alone must not invent creates |
| I. Ambiguous site | Same token many times; only one site is correct | Gold match via line anchors |
| J. Chain without re-read | Second edit uses post-edit preview TAG + lines | Exact file match |

## Tool sets

| Set | Tools |
| --- | --- |
| `hashline` | `read_file`, `edit`, `write_file`, `grep`, `glob`, `list_dir` |
| `rewrite` | `read_file`, `write_file`, `grep`, `glob`, `list_dir` |

## Decision rules

| Outcome | Rule |
| --- | --- |
| Keep hashline `edit` | Multi-hunk and multi-file work is faster/more reliable than rewrite |
| Drop hashline views | Question 4 shows token cost does not pay for itself |
| Prefer write_file | Large rewrites or creates dominate the suite |
