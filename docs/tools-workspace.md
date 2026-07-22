# Tools and workspace

Rho uses the current working directory as the workspace for file reads, edits, and shell commands. Start the [interactive TUI](/interactive-tui) or [automation command](/automation-cli) from the repository or directory you want Rho to work in.

## Built-in tools

Rho currently ships these compiled-in workspace tools on all platforms:

```text
list_dir
read_file
write_file
edit_file
```

It also exposes the `skill` tool, a read-only `rho` harness diagnostics tool, web access tools with zero-config invocation, and one native shell tool for the current platform:

```text
rho                 inspect runtime identity, context, prompt sources, tools, or sanitized config
web_search          search with optional provider credentials and store snippets by default
fetch_content       fetch pages, GitHub URLs, local files, PDFs, and video targets
get_search_content  retrieve stored content from a prior web_search or fetch_content call
process             start, poll, or stop a managed background shell process
bash                macOS and Linux
powershell          Windows
```

Built-in skills that ship with the binary include `rho-diagnostics` for harness diagnostics and `rho-agent-creator` for defining new agents. The `rho-agent-creator` skill guides you through a step-by-step questionnaire to produce a valid agent Markdown file with YAML frontmatter and a prompt body. Custom user skills can be added under `~/.rho/skills/<name>/SKILL.md`, `~/.agents/skills/<name>/SKILL.md`, or `<project-root>/.agents/skills/<name>/SKILL.md`. Set `disable-model-invocation: true` in a skill's frontmatter to prevent the model from loading it while keeping it available through `/skill:<name>`.

Web access tools keep normal prompts small. They return concise previews, snippets, citations or warnings when available, and response handles for stored content. `web_search` uses optional provider credentials for live results and stores fetched source pages only when `includeContent` succeeds. GitHub repository URLs prefer a local clone so the agent can inspect real files; oversized repositories fall back to the GitHub API unless `forceClone` is set. HTTP fetches refuse private, loopback, and link-local destinations by default. Set `RHO_SSRF_ALLOW_RANGES` to a comma-separated list of CIDRs (for example `198.18.0.0/15`) only when a TUN or fake-IP proxy requires it.

These tools can read and modify files, run shell commands in the working directory, and fetch external or local content when invoked. The `rho` tool is read-only and returns compact live snapshots. Its detailed action reference is embedded in the `rho-diagnostics` skill and loaded only when needed; diagnostics exclude credentials, prompt contents, and conversation history. Restart-only settings report the values used by the running process, not newer values saved for the next session.

## Image previews

`read_file` accepts PNG, JPEG, GIF, and WebP files in addition to UTF-8 text. Image files are decoded under strict byte, dimension, and allocation limits on a blocking worker, then reduced to a bounded PNG thumbnail. The immutable thumbnail is attached to the completed tool result, so later workspace changes cannot alter the preview. In the interactive TUI, the thumbnail renders directly in the feed on Kitty and Ghostty. Conservative capability detection avoids probing terminal input and keeps persistent tmux sessions on the text fallback because their terminal-specific environment can describe a stale client. Other terminals keep the normal text tool result without emitting graphics escape sequences. Image previews are presentation-only and are not restored when resuming a saved transcript.

## Atomic file edits

`edit_file` accepts either the existing single-edit arguments or an `edits` array for several exact replacements across one or more files. Array edits run in order, including when several edits target the same file. Each edit may set `expected_match_count` (default `1`); the edit fails as missing when fewer matches are found or ambiguous when more matches are found. Rho validates every replacement against in-memory file contents before writing any file, so a validation failure leaves all targeted files unchanged.

```json
{
  "edits": [
    {
      "path": "src/first.rs",
      "old_string": "old_name",
      "new_string": "new_name",
      "expected_match_count": 2
    },
    {
      "path": "src/second.rs",
      "old_string": "old call",
      "new_string": "new call"
    }
  ]
}
```

## Managed background processes

The `process` tool has three actions. `start` launches a background shell command and returns its process ID; it accepts an optional timeout. `poll` requires a process ID and returns retained stdout and stderr, optionally continuing from a cursor or waiting briefly for changes. Continue from the returned `next_cursor` to avoid duplicate output. Retention is bounded, so sufficiently old output can be discarded; poll results report when a requested cursor predates the retained range. `stop` requires a process ID and terminates the managed process tree.

Rho owns these processes only within the running instance. It cleans them up when that instance shuts down, and process records do not persist across restarts. The tool does not support stdin writes, process listing, pseudo-terminals, persistent sessions, or pane and session orchestration. Use a dedicated multiplexer such as tmux or Herdr when you need interactive terminals or persistent, orchestrated sessions.

Managed processes use standard output and error pipes, with standard input closed. Commands that require interactive input or terminal emulation will not behave as they do in a foreground terminal. The tool executes shell commands with the same user permissions as Rho. Rho's [permission modes](/configuration#permission-modes) can deny or request approval before process execution, but they do not add operating-system sandboxing.

## File writes and diffs

File write results include a unified diff so the model and transcript can inspect what changed. In the interactive TUI, added lines are highlighted in green, removed lines in red, and diff headers in the accent color. This is useful in both the [interactive TUI](/interactive-tui) and [automation mode](/automation-cli).

## Security and workspace boundaries

Tools run with the current user's permissions and can read or modify files and execute shell commands in the current workspace. The default `auto` [permission mode](/configuration#permission-modes) preserves this unrestricted behavior. `plan` denies file writes and process execution, while `supervised` asks for interactive confirmation before those operations. Supervised non-interactive runs fail closed because no approval UI is available.

Permission modes are policy checks at Rho's tool-capability boundary, not an operating-system sandbox. They do not reduce the permissions of the Rho process itself, and they depend on tools correctly declaring and authorizing capabilities. Run Rho only in workspaces where you are comfortable with the selected mode and these limitations.

For session storage separate from the workspace, see [sessions](/sessions). For output-size settings, see [configuration](/configuration#tool-output-limit).
