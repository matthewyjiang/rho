# Interactive TUI

Run `rho` in a terminal to start an interactive coding session in the current directory.

```bash
rho
```

The TUI is the main way to use Rho. Ask it to inspect files, explain code, make changes, run commands, or iterate on a task with you. Rho uses the current directory as its [workspace](/tools-workspace).

## Start a session

Open a project and run Rho from the repository root:

```bash
cd path/to/project
rho
```

Rho streams the assistant response as it works. Tool use appears inline so you can see commands, file reads, and edits as they happen. Completed conversation output remains in your normal terminal scrollback, so you can use your terminal's usual scrolling and copy behavior.

For persisted history and resume behavior, see [sessions](/sessions).

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

## Commands

Type `/` at the start of the message box to open the command palette. Keep typing to filter commands, use `up` and `down` to select, press `tab` to complete the selected command, and press `enter` to run it. Slash commands run locally instead of being sent to the AI.

| Command | Action |
| --- | --- |
| `/login` | Show [authentication](/authentication-and-models) help. A full login flow is not implemented yet. |
| `/model [provider/model]` | Open an auth-filtered [model picker](/authentication-and-models#providers-and-model-catalog), or switch directly to a provider/model and save it to [configuration](/configuration). |
| `/resume [id]` | Show [session resume](/sessions) help. Interactive session switching/listing is not implemented yet. |
| `/config` | Show the [config](/configuration) path and current key settings. A full config UI is not implemented yet. |
| `/exit` | Quit the TUI. |

A single `/` as the first character opens the command palette. Any later `/` characters are treated as normal message text and do not reopen the palette.

Some commands can replace the message box with a picker. Use `up` and `down` to select, type to filter by case-insensitive regex, press `tab` to autocomplete the filter from the highlighted item, press `enter` to confirm, and press `esc` to cancel.

## Model picker

The model picker is populated from Rho's built-in static catalog entries that match the active auth mode. `openai` uses API-key auth models, while `openai-codex` uses Codex auth models.

Use `/model provider/model` to switch explicitly, including to a provider outside the current picker filter:

```text
/model openai/gpt-5.5
/model openai-codex/gpt-5.5
```

A bare model id works when it uniquely matches the catalog. Uncataloged bare model ids stay on the current provider as an escape hatch for newly released models.

For provider and auth details, see [authentication and models](/authentication-and-models).

## Interrupt, reset, or quit

- Press `esc` to interrupt the current response without closing Rho.
- Press `ctrl-r` to reset the conversation history. The next message starts a new [session](/sessions).
- Press `ctrl-c` to clear the current input line.
- Press `ctrl-c` twice to quit.

## Useful controls

Most editing keys work the way they do in a normal terminal input.

| Key | Action |
| --- | --- |
| `esc` | Interrupt the current response, or hide the command palette when it is open |
| `/` at start | Open the command palette |
| `up` / `down` | Select a command while the palette is open |
| `tab` | Complete the selected command while the palette is open |
| `enter` | Send a prompt, or run a selected slash command |
| `ctrl-r` | Reset conversation history |
| `ctrl-c` | Clear input, then quit if pressed again |

Use [automation and CLI](/automation-cli) when you want a single answer outside the TUI.
