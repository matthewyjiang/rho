# Tools and workspace

Rho uses the current working directory as the workspace for file reads, edits, and shell commands. Start the [interactive TUI](/interactive-tui) or [automation command](/automation-cli) from the repository or directory you want Rho to work in.

## Built-in tools

Rho currently ships five compiled-in tools:

```text
list_dir
read_file
write_file
edit_file
bash
```

These tools can read and modify files and run shell commands in the working directory.

## File writes and diffs

File write results include a unified diff so the model and transcript can inspect what changed. This is useful in both the [interactive TUI](/interactive-tui) and [automation mode](/automation-cli).

## Sandbox and approvals

Rho does not currently sandbox or prompt for approval before tool calls. Run Rho only in workspaces where you are comfortable allowing file changes and shell commands.

For session storage separate from the workspace, see [sessions](/sessions). For output-size settings, see [configuration](/configuration#tool-output-limit).
