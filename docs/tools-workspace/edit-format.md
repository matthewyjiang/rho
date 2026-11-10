# Edit formats

Parent: [Tools and workspace](/tools-workspace).

Rho exposes exactly one file edit tool, chosen by
[`behavior.edit_tool`](/configuration#edit-tool). Each format is registered
under its own tool name:

| Format | Tool | Shape |
| --- | --- | --- |
| `str_replace` | `str_replace` | Exact `old_string` to `new_string` replacement in one existing file |
| `apply_patch` | `apply_patch` | Codex-style patch document with add, delete, update, and move sections across files |

Use the edit tool for targeted changes to existing UTF-8 files. Use `write` to
create a path or replace a whole file. Do not use shell or Python to rewrite
UTF-8 sources that the edit tool can express.

## `str_replace`

`str_replace` takes `path`, `old_string`, `new_string`, and an optional
`replace_all`. By default `old_string` must match exactly once; with
`replace_all` every match is replaced. Matching treats CRLF and LF newlines as
equal, and the write keeps the file's existing newline style.

## `apply_patch`

`apply_patch` takes one document in `input`:

```text
*** Begin Patch
*** Update File: src/app.py
@@
-print("hi")
+print("Hello, world!")
*** End Patch
```

Sections start with `*** Add File:`, `*** Delete File:`, or `*** Update File:`
(optionally followed by `*** Move to:`). Paths may be absolute or relative to
the working directory. The selected permission mode checks every affected
path, including both ends of a move, before any file changes. `Add File`
targets must not exist.

## Reads and results

`read_file` returns numbered `N:line` rows under a plain path header, and
`grep` content mode groups `N | text` match previews under each path. Rich
documents and images are not text-editable; see
[documents and images](/tools-workspace/documents-and-images).

Unified diffs are tool metadata for UI cards. They are not repeated in
model-facing content. In the interactive TUI, added and removed lines wash
toward the theme's green/red when RGB is available. Unhighlighted tokens sit on
that wash, or use the add/remove color if there is no wash. Signs stay
theme-colored, syntax roles keep their colors, and diff headers use the accent
color.

## Removed `hashline` format

Rho 3.0 removed the legacy `hashline` format and its `edit` tool. A config
that still sets `edit_tool = "hashline"`, under `[behavior]` or a
`[providers.custom.<name>]` table, is a load error naming the key. Saved sessions that contain `edit` calls
still resume, and those calls render as generic tool cards.

## Related

- [Search tools](/tools-workspace/search) - `grep` content mode previews
- [Tools and workspace](/tools-workspace) - when to prefer an edit vs `write`
