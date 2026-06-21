# Interactive TUI

Run `rho` in a terminal to start an interactive coding session in the current directory.

```bash
rho
```

The TUI is the main way to use Rho. Ask it to inspect files, explain code, make changes, run commands, or iterate on a task with you.

## Start a session

Open a project and run Rho from the repository root:

```bash
cd path/to/project
rho
```

Rho uses the current working directory as the workspace for file reads, edits, and shell commands.

Sessions persist automatically under `~/.rho/sessions/<workspace-key>/`, where `<workspace-key>` contains a readable encoding of the absolute working directory plus a stable hash to avoid path collisions. Starting `rho` creates a new session file only after you send the first message. To resume an existing session for the current workspace, pass its UUID or UUID prefix with `--resume` / `-R`:

```bash
rho --resume <session-uuid>
rho -R <session-uuid-prefix>
```

When Rho exits, it prints a resume command you can paste later. Pressing `ctrl-r` resets the conversation and starts a new session file.

## Send prompts

Type a request and press `enter` to send it.

Examples:

```text
summarize this repository
```

```text
add tests for the config parser
```

```text
find where the TUI handles paste events
```

Use a multiline prompt when you need to paste or write a longer request.

## Review output

Rho streams the assistant response as it works. Tool use appears inline so you can see commands, file reads, and edits as they happen.

Completed conversation output remains in your normal terminal scrollback, so you can use your terminal's usual scrolling and copy behavior.

## Interrupt work

Press `esc` to interrupt a response that is still running. This stops the current turn without closing Rho.

## Reset or quit

- Press `ctrl-r` to reset the conversation history.
- Press `ctrl-c` to clear the current input line.
- Press `ctrl-c` twice to quit.

## Useful controls

Most editing keys work the way they do in a normal terminal input. The controls worth knowing are:

| Key | Action |
| --- | --- |
| `esc` | Interrupt the current response |
| `ctrl-r` | Reset conversation history |
| `ctrl-c` | Clear input, then quit if pressed again |

## Non-interactive use

For one-off prompts outside the TUI, use `rho run`:

```bash
rho run "summarize this repository"
printf 'summarize this repository' | rho run --stdin
rho run "review this diff" --stdin < diff.txt
```

Use the TUI when you want an interactive session. Use `rho run` when you want a single answer for a script or terminal pipeline.
