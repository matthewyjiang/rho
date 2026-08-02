# Tools and workspace

Rho uses the current working directory as the workspace and as the base for relative file paths and shell commands. File paths can point outside that directory, either with parent components such as `../` or with absolute paths. Start the [interactive TUI](/interactive-tui) or [automation command](/automation-cli) from the repository or directory you want Rho to use as its main work context.

## Built-in tools

Rho currently ships these compiled-in workspace tools on all platforms:

```text
list_dir
read_file
write_file
edit_file
apply_patch
grep
glob
```

`grep` searches file contents with a regex. `glob` lists files whose paths match a pattern. Both run in-process and do not need `rg`, `fd`, or `rtk`.

- Patterns: `grep` takes a Rust/`regex` pattern. `glob` takes a path glob; a pattern with no `/` (for example `*.rs`) matches nested paths as `**/*.rs`.
- Defaults: both honor `.gitignore`, skip hidden files, and never follow symlinks. Pass `include_hidden` when you need dotfiles.
- Order: results come back in walk order, sorted by name within each directory, so repeat runs agree and a capped result is the first N paths shown rather than an arbitrary sample.
- Caps: results are bounded (default 200). `grep` also caps matches per file and trims long lines. Every capped, timed-out, or cancelled search says so in its summary, including when it found nothing.
- Output: `grep` groups matches by file. Set `output_mode` to `files_with_matches` or `count` when you only need paths or tallies; default is `content`.
- Permissions: both request read access only, so they work in every permission mode, including `plan`.

It also exposes the `skill` tool, a read-only `rho` harness diagnostics tool, web access tools with zero-config invocation, and one native shell tool for the current platform:

```text
rho                 inspect runtime identity, context, prompt sources, tools, or sanitized config
web_search          when hosted = true and the chat path supports it, use provider-hosted search; otherwise use the configured backup backend and store snippets by default
fetch_content       fetch pages, GitHub URLs, local files, PDFs, and video targets
get_search_content  retrieve stored content from a prior web_search or fetch_content call
process             start, poll, or stop a managed background shell process
workflow            validate, freeze, run, inspect, cancel, or resume a durable workflow; run/resume complete via automatic parent notification
bash                macOS and Linux
powershell          Windows
```

When the active model provider is xAI, Rho attaches xAI's hosted `x_search` tool on every model turn as a provider amenity. That tool searches X (x.com) posts, users, and threads server-side. It is separate from `web_search`, which uses hosted provider web search when enabled and supported, otherwise the configured client backup backends. Hosted X Search is not part of the agent tool allowlist: restricted or empty tool sets still receive it while the session uses xAI. Switching an existing session to xAI adds it on the next turn, and switching away removes it. Hosted X Search activity appears in the run stream as typed `HostedToolActivity` events with `name: "x_search"`.

Built-in skills that ship with the binary include `rho-diagnostics` for harness diagnostics, `rho-config` for guiding users through configuring Rho, `rho-agent-creator` for defining new agents, and `rho-workflow-authoring` for writing [workflows](/workflows). The `rho-config` skill guides users through the `/config` browser, model and provider selection, credential storage, and direct config-file edits. The `rho-agent-creator` skill guides you through a step-by-step questionnaire to produce a valid agent Markdown file with YAML frontmatter and a prompt body. Custom user skills can be added under `~/.rho/skills/<name>/SKILL.md`, `~/.agents/skills/<name>/SKILL.md`, or `<project-root>/.agents/skills/<name>/SKILL.md`. Set `disable-model-invocation: true` in a skill's frontmatter to prevent the model from loading it while keeping it available through `/skill:<name>`.

The workflow runtime also registers `workflow_command` on its provider-free host tool registry. This internal tool carries one frozen process request through policy, hooks, and approval. It is host-only and never appears in a model tool list.

Web access tools keep normal prompts small when needed, but `fetch_content` returns a single target's readable body inline when it fits the tool output limit. Larger or multi-target results keep a `responseId` for `get_search_content`. Full bodies are stored as sidecar blobs under the active session folder (`.../<session>/web/` for new sessions, or a legacy `*.web/` companion beside flat transcripts), not in the session transcript and not as paths for `read_file`. `get_search_content` selectors must use the exact original query/prompt or URL from the prior tool result; free-text keyword queries are rejected with the available selectors listed. `web_search` stores snippets by default and stores fetched source pages only when `includeContent` succeeds. GitHub repository URLs prefer a local clone so the tool can return real tree/file contents through the web tools; oversized repositories fall back to the GitHub API unless `forceClone` is set. Do not open web-access cache directories with `read_file`. HTTP fetches refuse private, loopback, and link-local destinations by default. Set `RHO_SSRF_ALLOW_RANGES` to a comma-separated list of CIDRs (for example `198.18.0.0/15`) only when a TUN or fake-IP proxy requires it.

`read_file` and `fetch_content` share Rho's bounded document extractor. Along with UTF-8 text and source files, it extracts text-layer PDFs, DOCX documents, and XLSX, XLS, or ODS spreadsheets. `pdf-inspector` preserves PDF headings, lists, tables, links, and reading order as structured Markdown. Spreadsheet output also uses bounded Markdown tables. PDF extraction preflights Flate stream expansion, including object and cross-reference streams, against a 64 MiB budget and rejects chained or unbounded stream filters. Extraction warnings and truncation are included in tool output and metadata. Scanned PDFs without a text layer report a clear warning because OCR, archive recursion, PPTX, and native provider document parts are not included. Remote PDFs use the same pure-Rust extraction path instead of a placeholder.

These tools can read and modify files inside or outside the workspace, run shell commands that start in the working directory, and fetch external or local content when invoked. The `rho` tool is read-only and returns compact live snapshots. Its detailed action reference is embedded in the `rho-diagnostics` skill and loaded only when needed; diagnostics exclude credentials, prompt contents, and conversation history. Restart-only settings report the values used by the running process, not newer values saved for the next session.

## Document extraction and image previews

Document extraction enforces a 25 MiB source limit and a 200,000-character extracted-text limit. Office and PDF parsers run behind the small `rho_tools::document` facade and optional crate features. Rho does not depend on `markitdown-rs` and does not unpack arbitrary archives.

`read_file` accepts PNG, JPEG, GIF, and WebP files in addition to text and supported documents. Image files are decoded under strict byte, dimension, and allocation limits on a blocking worker, then reduced to a bounded PNG thumbnail. The immutable thumbnail is attached to the completed tool result, so later workspace changes cannot alter the preview. In the interactive TUI, the thumbnail renders directly in the feed on Kitty and Ghostty. Under Herdr, Rho probes whether the active client can paint Kitty placements; if host cell metrics are unavailable, it falls back to halfblock previews so reserved feed rows are not left blank. Conservative capability detection avoids probing terminal input and keeps persistent tmux sessions on the text fallback because their terminal-specific environment can describe a stale client. Other terminals keep the normal text tool result without emitting graphics escape sequences. Image previews are presentation-only and are not restored when resuming a saved transcript.

## File edits

`edit_file` performs one string replacement in an existing UTF-8 file. Pass `path`, `old_string`, and `new_string`. Matching normalizes CRLF and LF line endings rather than requiring byte-exact newline matches, and the replacement is rewritten to preserve the file's existing newline style. By default `old_string` must match exactly once; set `replace_all` to replace every match. The tool opens the target under an exclusive lock for plan and write so concurrent modifications cannot be overwritten after validation. It fails closed when the match count is wrong, the file is missing, or the lock/write cannot complete. Successful results include a unified diff.

```json
{
  "path": "src/app.py",
  "old_string": "print(\"Hi\")",
  "new_string": "print(\"Hello, world!\")"
}
```

Use `edit_file` for a single surgical string replace. Use `apply_patch` for multi-hunk or multi-file edits. Use `write_file` to create or fully rewrite a file.

## File patches

`apply_patch` edits existing files and can also add or delete files with a Codex-style patch document. Pass the full patch text in `input`, including the `*** Begin Patch` and `*** End Patch` markers. Operations use `*** Add File:`, `*** Delete File:`, and `*** Update File:` headers. Update hunks use `@@` context markers and lines prefixed with ` ` (context), `-` (remove), or `+` (add). An update may include `*** Move to:` to rename a file while patching it.

Rho parses the whole patch, plans every file operation against current contents, rejects overlapping path claims, re-reads those files immediately before writing, and fails closed if any changed mid-flight. A planning or revalidation failure leaves all targeted files unchanged. If a later write fails after earlier writes succeeded, Rho rolls back the applied ops. Successful results include a unified diff of the committed changes.

```json
{
  "input": "*** Begin Patch\n*** Add File: hello.txt\n+Hello world\n*** Update File: src/app.py\n@@ def greet():\n-print(\"Hi\")\n+print(\"Hello, world!\")\n*** Delete File: obsolete.txt\n*** End Patch\n"
}
```

Use `write_file` when you need to create or fully replace a file with complete contents. Prefer `edit_file` for one surgical string replace. Use `apply_patch` for multi-hunk or multi-file edits.

## Managed background processes

The `process` tool has three actions. `start` launches a background shell command and returns its process ID; it accepts an optional timeout. `poll` requires a process ID and returns retained stdout and stderr, optionally continuing from a cursor or waiting briefly for changes. Continue from the returned `next_cursor` to avoid duplicate output. Retention is bounded, so sufficiently old output can be discarded; poll results report when a requested cursor predates the retained range. `stop` requires a process ID and terminates the managed process tree.

Rho owns these processes only within the running instance. It cleans them up when that instance shuts down, and process records do not persist across restarts. The tool does not support stdin writes, process listing, pseudo-terminals, persistent sessions, or pane and session orchestration. Use a dedicated multiplexer such as tmux or Herdr when you need interactive terminals or persistent, orchestrated sessions.

Managed processes use standard output and error pipes, with standard input closed. Commands that require interactive input or terminal emulation will not behave as they do in a foreground terminal. The tool executes shell commands with the same user permissions as Rho. Rho's [permission modes](/configuration#permission-modes) can deny or request approval before process execution, but they do not add operating-system sandboxing.

## File writes and diffs

File write results include a unified diff so the model and transcript can inspect what changed. In the interactive TUI, added lines are highlighted in green, removed lines in red, and diff headers in the accent color. This is useful in both the [interactive TUI](/interactive-tui) and [automation mode](/automation-cli).

## Security and workspace boundaries

Tools run with the current user's permissions. File tools can read or modify any path that the user can access, including paths outside the workspace, and shell commands can do the same. The default `auto` [permission mode](/configuration#permission-modes) allows this behavior. `plan` denies file writes and process execution, while `supervised` asks for interactive confirmation before those operations. Supervised non-interactive runs fail closed because no approval UI is available.

Permission modes are policy checks at Rho's tool-capability boundary, not an operating-system sandbox. They do not reduce the permissions of the Rho process itself, and they depend on tools correctly declaring and authorizing capabilities. The SDK still scopes file access by default; embedded hosts must opt into broader access when they build a `Workspace`. Run Rho only in workspaces where you are comfortable with the selected mode and these limits.

For session storage separate from the workspace, see [sessions](/sessions). For output-size settings, see [configuration](/configuration#tool-output-limit).
