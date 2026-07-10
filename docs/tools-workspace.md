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

It also exposes the `skill` tool, web access tools with zero-config invocation, and one native shell tool for the current platform:

```text
web_search          search with optional provider credentials and store snippets by default
fetch_content       fetch pages, GitHub URLs, local files, PDFs, and video targets
get_search_content  retrieve stored content from a prior web_search or fetch_content call
bash                macOS and Linux
powershell          Windows
```

Web access tools keep normal prompts small. They return concise previews, snippets, citations or warnings when available, and response handles for stored content. `web_search` uses optional provider credentials for live results and stores fetched source pages only when `includeContent` succeeds. GitHub repository URLs prefer a local clone so the agent can inspect real files; oversized repositories fall back to the GitHub API unless `forceClone` is set.

These tools can read and modify files, run shell commands in the working directory, and fetch external or local content when invoked.

## File writes and diffs

File write results include a unified diff so the model and transcript can inspect what changed. In the interactive TUI, added lines are highlighted in green, removed lines in red, and diff headers in the accent color. This is useful in both the [interactive TUI](/interactive-tui) and [automation mode](/automation-cli).

## Sandbox and approvals

Rho does not currently sandbox or prompt for approval before tool calls. Run Rho only in workspaces where you are comfortable allowing file changes and shell commands.

For session storage separate from the workspace, see [sessions](/sessions). For output-size settings, see [configuration](/configuration#tool-output-limit).
