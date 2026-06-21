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

After you send at least one message, Rho prints a resume command on exit that you can paste later. Pressing `ctrl-r` resets the conversation; the next message starts a new session file.

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

## Commands

Type `/` at the start of the message box to open the command palette. Keep typing to filter commands, use `up` and `down` to select, press `tab` to complete the selected command, and press `enter` to run it. Slash commands run locally instead of being sent to the AI.

Initial commands:

| Command | Action |
| --- | --- |
| `/login` | Show authentication help. A full login flow is not implemented yet. |
| `/model [provider/model]` | Open Rho's static cross-provider model catalog, or switch directly to a provider/model and save it to config. |
| `/resume [id]` | Show resume help. Interactive session switching/listing is not implemented yet. |
| `/config` | Show the config path and current key settings. A full config UI is not implemented yet. |
| `/exit` | Quit the TUI. |

A single `/` as the first character opens the command palette. Any later `/` characters are treated as normal message text and do not reopen the palette. Some commands can replace the message box with a picker; use `up`/`down` to select, `enter` to confirm, and `esc` to cancel.

The model picker is populated from Rho's built-in static catalog for all implemented providers. `openai` uses API-key auth models, while `openai-codex` uses Codex auth models. Use `/model provider/model` to switch explicitly, for example `/model openai/gpt-5.5` or `/model openai-codex/gpt-5.5`. A bare model id works when it uniquely matches the catalog; uncataloged bare model ids stay on the current provider as an escape hatch for newly released models.

## Reset or quit

- Press `ctrl-r` to reset the conversation history.
- Press `ctrl-c` to clear the current input line.
- Press `ctrl-c` twice to quit.

## Useful controls

Most editing keys work the way they do in a normal terminal input. The controls worth knowing are:

| Key | Action |
| --- | --- |
| `esc` | Interrupt the current response, or hide the command palette when it is open |
| `/` at start | Open the command palette |
| `up` / `down` | Select a command while the palette is open |
| `tab` | Complete the selected command while the palette is open |
| `enter` | Send a prompt, or run a selected slash command |
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
