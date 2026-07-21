# Interactive TUI

Run `rho` in a terminal to start an interactive coding session in the current directory.

```bash
rho
```

The TUI is the main way to use Rho. Ask it to inspect files, explain code, make changes, run commands, or iterate on a task with you. Rho uses the current directory as its [workspace](/tools-workspace). Tool access and command execution follow the workspace and security behavior described in [tools and workspace](/tools-workspace#security-and-workspace-boundaries).

## Start a session

Open a project and run Rho from the repository root:

```bash
cd path/to/project
rho
```

Rho streams the assistant response as it works. Tool use appears inline so you can see commands, file reads, and edits as they happen. Markdown ATX headings from `#` through `######` render without their syntax markers, using distinct terminal colors and stronger emphasis for the top three levels. Provider streams that deliver no data for two minutes are treated as stale, so Rho can reset or surface an error instead of remaining in the `working` state indefinitely. The interactive UI owns the transcript viewport while it is open, so use the built-in transcript scrolling controls instead of terminal scrollback. When you exit, your previous shell view returns and Rho prints only a short saved-session summary when a session exists.

For persisted history and resume behavior, see [sessions](/sessions).

### Mermaid diagrams

Closed fenced code blocks whose first info token is `mermaid` render as terminal-native Unicode diagrams. The match is case-insensitive and extra info tokens are allowed. During streaming, an open fence remains a normal source code block and changes to diagram art only when its closing fence arrives. The diagram is laid out again when the terminal width changes.

Rho uses `mermaid-rs-renderer` 0.3.1 as its Mermaid parser and semantic model. The terminal painter provides quality-first support for core subsets of flowcharts and graphs, state diagrams, sequence diagrams, class diagrams, and entity-relationship diagrams. Other diagram families and constructs the painter cannot represent losslessly remain raw code blocks, as do unsupported syntax and malformed input. This is not full Mermaid.js syntax or visual parity.

Unsupported, malformed, unsafe, oversized, or too-wide diagrams silently remain normal code blocks. Rendering does not execute links or scripts, requires no external executable or network access, and does not trust Mermaid-provided terminal styles. The panel's `COPY` action copies the original Mermaid source rather than the rendered box art.

## Watch a subagent

Run `rho attach <id>` to watch a subagent reported by the `agent` tool:

```bash
rho attach abc123
```

Attached mode uses a separate read-only TUI. It renders the delegated prompt,
reasoning, assistant output, tool activity, usage, and final state, but it has no
message box and cannot submit prompts or change the subagent environment. Use
Up/Down, Page Up/Page Down, and Home/End to scroll. Press `q`, Escape, or Ctrl-C
to detach without stopping the run. See [subagents](/subagents#watching-a-subagent)
for lifecycle and Herdr behavior.

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

Type `/` at the start of the message box to open the command palette. Keep typing to filter commands, use `up` and `down` to select, press `tab` to complete the selected command, and press `enter` to run it. Most built-in slash commands run locally. Commands that start agent work say so below.

| Command | Action |
| --- | --- |
| `/login [provider]` | Log in with a provider. No args opens a picker; direct args target a single [provider](/authentication-and-models#providers). |
| `/logout [provider]` | Delete stored provider credentials. No args opens a picker; direct args target a single [provider](/authentication-and-models#providers). |
| `/model [provider/model]` | Open a picker for models with available auth, or switch directly to a provider/model and save it to [configuration](/configuration). Press `ctrl-p` in the picker to pin or unpin the highlighted model. |
| `/resume [id]` | Resume a saved session by UUID or prefix. No args opens a picker for other sessions in the current workspace. |
| `/config` | Open the [config](/configuration) category browser for models and reasoning, agent behavior, context limits, tools, providers, and updates. |
| `/info` | Show the running Rho version, provider, model, reasoning level, and permission mode. |
| `/compact` | Immediately summarize older conversation history to reduce future model context. This works even when auto compaction is disabled. |
| `/goal [condition]` | Set a completion condition and start working immediately. Rho explicitly tells the agent that this is a goal-setting action, then evaluates the transcript after each turn and continues until the condition is met. Connection errors and other incomplete runs are retried automatically while the goal remains active. If only steps requiring user authority remain, the goal pauses as blocked and reports those steps. Run `/goal` for status, `/goal resume` after completing blocked steps, or `/goal clear` to cancel. |
| `/skills` | Show available workspace skills and insert a `/skill:<name>` command for one. Running the inserted command loads the skill through the skill tool before the model responds. Add text after the command to include extra instructions in the same turn. |
| `/agents` | Reload agent definitions and browse their descriptions, sources, model policies, reasoning levels, tools, prompt policies, and prompt previews. Select a reserved internal agent to configure its model. |
| `/diff` | Show local Git status plus staged and unstaged worktree patches without invoking the model. |
| `/doctor` | Check provider authentication, the selected model, config and session writability, model caches, clipboard image helpers, rtk, and Herdr integration without displaying secrets. |
| `/limits` | Fetch and show the usage windows reported by connected OAuth providers. Codex OAuth, Kimi Code OAuth, and xAI OAuth are supported when logged in; absent windows are omitted. |
| `/export [path]` | Export the current session to a self-contained HTML transcript. Assistant Markdown, including inline `$...$` or `\(...\)` and display `$$...$$` or `\[...\]` LaTeX math, is rendered in the exported file. |
| `/exit` | Quit the TUI. |

Custom prompt templates loaded from prompt files or [`[prompt_templates]`](/configuration#prompt-templates) also appear in the command palette. Completing one inserts its prompt into the composer so you can add or edit text before sending.

A single `/` as the first character opens the command palette. Any later `/` characters are treated as normal message text and do not reopen the palette. While a goal is active, the status line shows an `◎ /goal active` indicator with the evaluated turn count and elapsed time. A goal paused for user action shows `◎ /goal blocked`; sending a new message or running `/goal resume` asks the agent to verify the blocked steps before continuing implementation work.

Some commands can replace the message box with a picker. Use `up` and `down` to select, type to filter by case-insensitive regex, press `tab` to autocomplete the filter from the highlighted item, press `enter` to confirm, and press `esc` to cancel. In conversation and internal-agent model pickers, press `ctrl-p` to pin or unpin the highlighted model; pinned models are saved in config and shown first in both picker types. `/config` starts with a short category browser. Its search matches the settings listed inside each category. Press `enter` to open a category and `esc` to return. Press `space` on an on/off setting to toggle it in place. Changes save at once and return to the same category so you can keep adjusting its settings; login workflows close the picker while credentials are entered or authorized.

In supervised mode, a tool that wants to write a file or execute a process opens a dedicated approval prompt in the composer. Use the arrow keys to choose **Allow once**, **Allow for session**, or **Deny**, then press Enter. Long operation details open at the final page so dangerous command suffixes remain visible; use Page Up and Page Down to inspect every detail page without hiding the choices. Choosing **Deny** rejects that operation without ending the session. Press Escape to deny and cancel the current run. The active `plan` or `supervised` mode appears in the status line; the default `auto` mode stays hidden to avoid clutter.

Type `@` to open a workspace file picker. Keep typing to fuzzy-search paths, use `up` and `down` to select, then press `tab` or `enter` to insert the highlighted path into the message as an `@path` reference. The picker follows `.gitignore`, `.ignore`, and global Git ignore rules while still showing hidden workspace files that are not ignored.

## Login and logout

`/login` opens a readable provider picker. Providers with multiple methods open a second picker such as **API Key** or **OAuth**; providers with one method continue directly to their login flow. Passing an internal provider name (for example `/login openai`) targets that method directly. Each flow is documented on the [provider page](/authentication-and-models#providers). Credentials are stored in the native OS credential store, not in config or transcripts.

`/logout` opens a provider picker containing only providers with stored credentials that can be deleted, or targets one directly (for example `/logout openai`). Environment overrides are CI/development hatches and can keep a provider available after logout.

Logging in does not normally switch provider/model. Use `/model` to switch models and providers. If Rho started without usable auth, a successful login selects that provider's default model so the session can run.

## Model picker

The model picker is populated from Rho's static catalog entries and cached dynamic provider model lists for providers that currently have auth available through `/login` or env overrides. Which models each provider exposes, and whether its list is refreshable, is covered on the [provider pages](/authentication-and-models#providers). Open `/config`, choose **Providers**, then choose **Refresh model lists** to fetch models for one or all refreshable providers when credentials are available. Press `ctrl-p` on a highlighted picker row to pin or unpin that model. Pinned models are stored in `favorite_models` in config and appear at the top of conversation and internal-agent model pickers in the order they were pinned.

Use `/model provider/model` to switch explicitly, including to a provider outside the current picker filter:

```text
/model openai/gpt-5.6-sol
/model openai-codex/gpt-5.6-sol
/model anthropic/claude-sonnet-4-5
/model github-copilot/gpt-4.1
```

A bare model id works when it uniquely matches the catalog. Uncataloged bare model ids stay on the current provider as an escape hatch for newly released models.

`/model` remains available while an agent run is active. You can browse the picker or select a model directly, but the current run continues using its existing model through all remaining model steps and tool calls. The queued model change is applied only after the full agent loop ends, before the next queued message starts. Selecting another model before then replaces the pending choice.

Run `/agents` to inspect reserved internal agents. The detail pane shows the effective provider/model and whether it follows the conversation or uses an override. Press Enter on `session-title` or `goal-judge` to choose a model. Select **Use conversation model** to remove that role's override. Each role resolves its own setting when invoked, so changing one does not affect the other.

For provider and auth details, see [authentication and models](/authentication-and-models).

## Interrupt, steer, reset, or quit

- Press `esc` to abort the current response without closing Rho. The provider request and active tool receive the same cancellation signal, partial assistant output remains in the session, and queued prompts are restored to the composer instead of running automatically.
- Press `enter` while Rho is working to steer the run. Rho finishes every tool call from the current assistant turn, adds their results to context, then inserts the steering message before the next model request.
- Press `ctrl-r` to reset the conversation history. The next message starts a new [session](/sessions).
- Press `ctrl-c` to clear the current input line.
- Press `ctrl-c` twice to quit.

## Useful controls

Most editing keys work the way they do in a normal terminal input.

| Key | Action |
| --- | --- |
| `esc` | Abort the current response and restore queued work, or hide the command palette when it is open |
| `/` at start | Open the command palette |
| `@` | Open workspace file path autocomplete |
| `up` / `down` | Re-enter previous prompts, or select a command or file while a picker is open |
| `tab` | Complete the selected command or file path |
| `enter` | Send a prompt, run a selected slash command, or steer after the current assistant turn while a response is running |
| `alt-up` | Pull the most recent queued prompt back into the composer for editing |
| `ctrl-r` | Reset conversation history |
| `pageup` / `pagedown` | Scroll the transcript viewport |
| `ctrl-g` | Jump the transcript viewport back to the bottom |
| mouse wheel | Scroll the transcript viewport |
| left-click and drag | Select transcript text and copy it on release |
| code block `COPY` | Copy the full code block contents |
| `ctrl-c` | Clear input, then quit if pressed again |

Copied text is sent to the terminal clipboard, and Rho briefly shows how many characters were copied. Code block copy buttons are shown in the top-right border and highlight on hover.

When the transcript is scrolled away from the bottom, Rho overlays a right-aligned `↓ jump to bottom  ctrl+g` button on the last transcript row and obscures only the button's own cells. During generation, the spinner is similarly overlaid on the left. At the live bottom, transcript content stops one row above the spinner; while manually scrolled, the complete last row remains visible wherever neither control is drawn. Press `ctrl-g` or click the button to resume following live output.

Use [automation and CLI](/automation-cli) when you want a single answer outside the TUI.
