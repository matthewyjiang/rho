# Inline shell

Run a local shell command from the interactive TUI without leaving your Rho session. Inline shell commands use the current workspace as their working directory and display their output in the transcript.

## Run a command

Type a command with `!` at the start of the composer, then press `enter`:

```text
!git status --short
```

Rho runs the command locally and adds the command, its output, and its exit status to the next model context. Use this mode when you want the model to inspect or act on the result.

Use `!!` when you want to see the result without sending it to the model:

```text
!!git diff --stat
```

Rho still displays the command and output in the transcript, but it excludes them from model context. Typing `!` or `!!` at an empty composer enters shell mode as explicit state: the composer stores only the command body, and the top border shows the mode label:

- `!`: top border labeled `shell · included in context`
- `!!`: top border labeled `shell · excluded from context`

Press `esc` to leave shell mode and return to the normal composer. The command text stays in the composer.

## Complete a path

Press `tab` in shell mode to complete the word under the cursor, one path component at a time, the way a shell does:

```text
!cat cra<tab>            → !cat crates/
!cat crates/rh<tab>      → !cat crates/rho/
!cat crates/rho/Ca<tab>  → !cat crates/rho/Cargo.toml
```

One match is inserted at once. A directory ends in `/` and gets no trailing space, so the next `tab` descends into it; a file gets a trailing space. Several matches open a list under the composer showing that directory's entries: keep typing to narrow it, use `up` and `down` to choose, then press `tab` or `enter` to insert the highlighted entry or `esc` to close the list and stay in shell mode. Paths that contain spaces or other characters the shell would interpret are single-quoted.

Completion lists the filesystem as the shell will see it, so untracked and ignored entries such as `target/` are offered. Hidden entries appear only when the component you are typing starts with `.`. Relative paths resolve from the workspace; `~/`, `../`, and absolute paths work as they would in the shell. Completion offers paths only; it does not complete command names or flags.

Rho runs inline commands asynchronously, so you can continue working while a command runs. Press `esc` to cancel a running command. Rho stops commands that run longer than 60 seconds.

## Choose a shell

Rho uses Bash on macOS and Linux, and PowerShell on Windows by default. Open `/config` in the TUI and select **Inline shell** to choose another detected shell.

Rho checks these shells when it builds the picker:

- macOS and Linux: `bash`, `zsh`, `fish`, and `sh`
- Windows: `powershell`, `pwsh`, and `cmd`

The picker lists shells that Rho finds on your `PATH`. It also keeps the configured value available when you use a custom executable path. Rho starts each command in the workspace directory with standard input closed.

## Security

Inline shell commands run with your user permissions. Rho doesn't sandbox them or ask for approval before execution, so review commands before you submit them. Run Rho only in workspaces where you trust the commands you enter.

For commands that the model chooses to run as tools, see [Tools and workspace](/tools-workspace). For the TUI's other input and command controls, see [Interactive TUI](/interactive-tui).
