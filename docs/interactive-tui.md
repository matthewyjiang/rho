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
| `/login [provider]` | Log in with a provider. No args opens a picker; direct args support `openai` and `openai-codex`. |
| `/logout [provider]` | Delete stored provider credentials. No args opens a picker; direct args support `openai` and `openai-codex`. |
| `/model [provider/model]` | Open a picker for models with available auth, or switch directly to a provider/model and save it to [configuration](/configuration). |
| `/resume [id]` | Show [session resume](/sessions) help. Interactive session switching/listing is not implemented yet. |
| `/config` | Open the [config](/configuration) picker. Reasoning changes apply immediately; reasoning output visibility applies on the next model call; max output bytes changes save for the next session. |
| `/exit` | Quit the TUI. |

A single `/` as the first character opens the command palette. Any later `/` characters are treated as normal message text and do not reopen the palette.

Some commands can replace the message box with a picker. Use `up` and `down` to select, type to filter by case-insensitive regex, press `tab` to autocomplete the filter from the highlighted item, press `enter` to confirm, and press `esc` to cancel. In `/config`, the picker stays open after changing a value so you can continue adjusting settings.

## Login and logout

`/login` opens a provider picker. `/login openai` opens a masked API-key entry box. `/login openai-codex` starts Rho's browser-based Codex OAuth flow. Credentials are stored in the native OS credential store, not in config or transcripts.

`/logout` opens the same provider picker and deletes credentials from the device. `/logout openai` deletes the stored OpenAI API key. `/logout openai-codex` deletes stored Codex tokens. Environment overrides are CI/development hatches and can keep a provider available after logout.

Logging in does not normally switch provider/model. Use `/model` to switch models and providers. If Rho started without usable auth, a successful login selects that provider's default model so the session can run.

## Model picker

The model picker is populated from Rho's built-in static catalog entries for providers that currently have auth available through `/login` or env overrides. `openai` uses API-key auth models, while `openai-codex` uses Codex auth models.

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
| `up` / `down` | Re-enter previous prompts, or select a command while the palette is open |
| `tab` | Complete the selected command while the palette is open |
| `enter` | Send a prompt, run a selected slash command, or queue a prompt while a response is running |
| `alt-up` | Pull the most recent queued prompt back into the composer for editing |
| `ctrl-r` | Reset conversation history |
| `ctrl-c` | Clear input, then quit if pressed again |

Use [automation and CLI](/automation-cli) when you want a single answer outside the TUI.
