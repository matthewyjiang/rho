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

Rho streams the assistant response as it works. Tool use appears inline so you can see commands, file reads, and edits as they happen. Provider streams that deliver no data for two minutes are treated as stale, so Rho can reset or surface an error instead of remaining in the `working` state indefinitely. The interactive UI owns the transcript viewport while it is open, so use the built-in transcript scrolling controls instead of terminal scrollback. When you exit, your previous shell view returns and Rho prints only a short saved-session summary when a session exists.

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
| `/login [provider]` | Log in with a provider. No args opens a picker; direct args support `openai`, `openai-codex`, `anthropic`, and `github-copilot`. |
| `/logout [provider]` | Delete stored provider credentials. No args opens a picker; direct args support `openai`, `openai-codex`, `anthropic`, and `github-copilot`. |
| `/model [provider/model]` | Open a picker for models with available auth, or switch directly to a provider/model and save it to [configuration](/configuration). Press `ctrl-p` in the picker to pin or unpin the highlighted model. |
| `/title-model [provider/model]` | Open a picker for the session-title model, or switch it directly and save optional title model settings. |
| `/refresh-model-list [provider]` | Refresh cached API model lists for a provider, or for all refreshable authenticated providers when no provider is given. |
| `/resume [id]` | Resume a saved session by UUID or prefix. No args opens a picker for other sessions in the current workspace. |
| `/config` | Open the [config](/configuration) picker. Reasoning changes apply immediately; reasoning output visibility and auto compaction settings apply on the next model call; max output bytes changes save for the next session. |
| `/compact` | Immediately summarize older conversation history to reduce future model context. This works even when auto compaction is disabled. |
| `/goal [condition]` | Set a completion condition and start working immediately. After each turn, Rho evaluates the transcript and continues until the condition is met. Run `/goal` for status or `/goal clear` to cancel. |
| `/skills` | Show loaded workspace skills and insert a `/skill:<name>` command for one. |
| `/diff` | Show local Git status plus staged and unstaged worktree patches without invoking the model. |
| `/doctor` | Check provider authentication, the selected model, config and session writability, model caches, clipboard image helpers, rtk, and Herdr integration without displaying secrets. |
| `/limits` | Fetch and show the usage windows reported by connected OAuth providers. Codex OAuth is currently supported; absent windows are omitted. |
| `/exit` | Quit the TUI. |

Custom prompt templates loaded from prompt files or [`[prompt_templates]`](/configuration#prompt-templates) also appear in the command palette. Completing one inserts its prompt into the composer so you can add or edit text before sending.

A single `/` as the first character opens the command palette. Any later `/` characters are treated as normal message text and do not reopen the palette. While a goal is active, the status line shows an `◎ /goal active` indicator with the evaluated turn count and elapsed time.

Some commands can replace the message box with a picker. Use `up` and `down` to select, type to filter by case-insensitive regex, press `tab` to autocomplete the filter from the highlighted item, press `enter` to confirm, and press `esc` to cancel. In `/model` and `/title-model`, press `ctrl-p` to pin or unpin the highlighted model; pinned models are saved in config and shown first in both model pickers. In `/config`, the picker stays open after changing a value so you can continue adjusting settings.

Type `@` to open a workspace file picker. Keep typing to fuzzy-search paths, use `up` and `down` to select, then press `tab` or `enter` to insert the highlighted path into the message as an `@path` reference. The picker follows `.gitignore`, `.ignore`, and global Git ignore rules while still showing hidden workspace files that are not ignored.

## Login and logout

`/login` opens a provider picker. `/login openai` and `/login anthropic` open masked API-key entry boxes. `/login openai-codex` starts Rho's browser-based Codex OAuth flow. `/login github-copilot` starts GitHub device code login for GitHub Copilot. Credentials are stored in the native OS credential store, not in config or transcripts.

`GITHUB_COPILOT_TOKEN` can be used as a CI/development bearer-token override without storing credentials.

`/logout` opens a provider picker containing only providers with stored credentials that can be deleted. `/logout openai` deletes the stored OpenAI API key. `/logout openai-codex` deletes stored Codex tokens. `/logout anthropic` deletes the stored Anthropic API key. `/logout github-copilot` deletes stored GitHub Copilot tokens. Environment overrides are CI/development hatches and can keep a provider available after logout.

Logging in does not normally switch provider/model. Use `/model` to switch models and providers. If Rho started without usable auth, a successful login selects that provider's default model so the session can run.

## Model picker

The model picker is populated from Rho's static catalog entries and cached dynamic provider model lists for providers that currently have auth available through `/login` or env overrides. `openai` uses API-key auth models, `openai-codex` uses Codex auth models, `anthropic` uses Anthropic API-key models, and `github-copilot` uses GitHub Copilot models. Run `/refresh-model-list github-copilot` to fetch Copilot models when credentials are available. Press `ctrl-p` on a highlighted picker row to pin or unpin that model. Pinned models are stored in `favorite_models` in config and appear at the top of `/model` and `/title-model` in the order they were pinned.

Use `/model provider/model` to switch explicitly, including to a provider outside the current picker filter:

```text
/model openai/gpt-5.5
/model openai-codex/gpt-5.5
/model anthropic/claude-sonnet-4-5
/model github-copilot/gpt-4.1
```

A bare model id works when it uniquely matches the catalog. Uncataloged bare model ids stay on the current provider as an escape hatch for newly released models.

`/model` remains available while an agent run is active. You can browse the picker or select a model directly, but the current run continues using its existing model through all remaining model steps and tool calls. The queued model change is applied only after the full agent loop ends, before the next queued message starts. Selecting another model before then replaces the pending choice.

Use `/title-model` to choose the model used for session title generation. The title model picker follows the same model catalog and auth availability rules as `/model`, but saves optional `title_provider`, `title_model`, and `title_auth` settings instead of changing the active chat model.

For provider and auth details, see [authentication and models](/authentication-and-models).

## Interrupt, reset, or quit

- Press `esc` to interrupt the current response without closing Rho. If a tool command is running, Rho terminates it and ends the turn immediately.
- Press `ctrl-r` to reset the conversation history. The next message starts a new [session](/sessions).
- Press `ctrl-c` to clear the current input line.
- Press `ctrl-c` twice to quit.

## Useful controls

Most editing keys work the way they do in a normal terminal input.

| Key | Action |
| --- | --- |
| `esc` | Interrupt the current response, or hide the command palette when it is open |
| `/` at start | Open the command palette |
| `@` | Open workspace file path autocomplete |
| `up` / `down` | Re-enter previous prompts, or select a command or file while a picker is open |
| `tab` | Complete the selected command or file path |
| `enter` | Send a prompt, run a selected slash command, or queue a prompt while a response is running |
| `alt-up` | Pull the most recent queued prompt back into the composer for editing |
| `ctrl-r` | Reset conversation history |
| `pageup` / `pagedown` | Scroll the transcript viewport |
| `ctrl-g` | Jump the transcript viewport back to the bottom |
| mouse wheel | Scroll the transcript viewport |
| left-click and drag | Select transcript text and copy it on release |
| code block `COPY` | Copy the full code block contents |
| `ctrl-c` | Clear input, then quit if pressed again |

Copied text is sent to the terminal clipboard, and Rho briefly shows how many characters were copied. Code block copy buttons are shown in the top-right border and highlight on hover.

When the transcript is scrolled away from the bottom, Rho shows a `↓ jump to bottom  ctrl+g` button directly above the composer. Press `ctrl-g` or click the button to resume following live output.

Use [automation and CLI](/automation-cli) when you want a single answer outside the TUI.
