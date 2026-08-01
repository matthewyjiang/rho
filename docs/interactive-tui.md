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

Flowcharts and state diagrams keep the direction you asked for. When their normal layout is wider than the pane, Rho wraps node labels more tightly and lays the diagram out again, down to a readable limit. Compaction never shortens or truncates label text.

Diagrams Rho cannot draw stay readable as source, and the panel border says why. A diagram that needs a wider pane reads `MERMAID · PANE TOO NARROW`, so you can widen the pane or the terminal to see the art. Everything else Rho declines to draw, such as unsupported, malformed, unsafe, or oversized input, reads `MERMAID · NOT RENDERED`. Very narrow panels drop the label so the `COPY` action keeps its place. Resizing moves a diagram between art and source in both directions.

Rendering does not execute links or scripts, requires no external executable or network access, and does not trust Mermaid-provided terminal styles. The panel's `COPY` action copies the original Mermaid source rather than the rendered box art, for both diagrams and source fallbacks.

## Watch a subagent

Run `rho attach <id>` to watch a subagent reported by the `agent` tool:

```bash
rho attach abc123
```

Attached mode uses a separate read-only TUI. It renders the delegated prompt,
reasoning, assistant output, tool activity, usage, and final state, but it has no
message box and cannot submit prompts or change the subagent environment. Use
Up/Down, Page Up/Page Down, and Home/End to scroll. Press `q`, Escape, or Ctrl-C
to detach without stopping the run. For Claude-cli runs, attach also surfaces
`claude_session_id` when present so you can open the full Claude transcript with
`claude --resume <session-id>`. See [subagents](/subagents#attachment-and-artifacts)
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

Press `ctrl+v` to paste a clipboard image as an attachment when a supported host helper is available (`wl-paste`/`xclip` on Linux, `pngpaste` on macOS, or PowerShell on Windows/WSL). Hosts such as Herdr may paste clipboard content as a single filesystem path. Rho loads PNG, JPEG, GIF, and WebP paths as image attachments. It also extracts text from UTF-8 text and source files, PDFs, DOCX documents, and XLSX, XLS, or ODS spreadsheets and queues the result as a document attachment. An absolute document path is handled before slash-command parsing, so paths beginning with `/` do not become unknown commands. Press backspace in an empty message box to remove the last queued file.

Document extraction is bounded by input and extracted-character limits. PDFs need a text layer because scanned-image OCR is not included. The model receives extracted text with the filename, MIME type, truncation state, and warnings. Session model history stores that bounded text and metadata, not raw PDF or Office bytes. Images continue to use the provider's multimodal image path.

## Commands

Type `/` at the start of the message box to open the command palette. Keep typing to filter commands, use `up` and `down` to select, press `tab` to complete the selected command, and press `enter` to run it. Most built-in slash commands run locally. Commands that start agent work say so below.

| Command | Action |
| --- | --- |
| `/login [provider]` | Log in with a provider or the Claude Code runtime. No args opens a picker (Claude Code is under **Anthropic** as **Claude Code (delegation only)**); direct args target a single [provider](/authentication-and-models#providers) or `/login claude-code`. |
| `/logout [provider]` | Delete stored provider credentials, or sign out of Claude Code everywhere with `/logout claude-code` (after confirmation). No args opens a picker; direct args target a single [provider](/authentication-and-models#providers). |
| `/model [provider/model]` | Open a picker for models with available auth, or choose a provider/model and save it to [configuration](/configuration). When switching would drop provider-native context, or when the current model has completed a live turn and older context can be compacted, Rho asks how to continue. Compaction can summarize portable context first; it does not make native blocks sendable to the new model. Press `ctrl-p` in the picker to pin or unpin the highlighted model. |
| `/fast [on\|off]` | Toggle or set the faster priority tier for supported Codex models. Fast mode saves to configuration, appears as `(fast)` after the model name, and uses credits at a higher rate. |
| `/resume [id]` | Resume a saved session by UUID or prefix. No args opens a picker for other sessions in the current workspace. In the picker, press `d` or `Delete` to remove a session after confirmation. If the current model cannot use the session's provider-native context, Rho asks whether to resume with the session model, compact with that model first, or continue on the current model. |
| `/tree` | Navigate completed turns and compaction states in the current session. Continuing from an older state creates a branch. |
| `/workflow` | Open the workflow list. Start a local workflow or plan in the background (run id is appended to chat context), watch a run on the DAG screen, reuse a saved plan, or press `d` to delete a plan/run. Keep chatting while a run continues; completion is delivered automatically. Reopen `/workflow` and Enter a run to watch. |
| `/rewind [turn]` | Preview and restore native file-tool changes from a completed turn, then continue from that conversation state on a new branch. This experimental command requires `behavior.experimental_workspace_rewind = true`. It does not reverse shell, Git, process, network, database, or service effects. Conflicting paths stay unchanged. |
| `/config` | Open the [config](/configuration) category browser for models and reasoning, agent behavior, context limits, tools, providers, and updates. |
| `/info` | Show the running Rho version, provider, model, reasoning level, permission mode, and external runtime status (including Claude Code ownership). |
| `/compact` | Immediately summarize older conversation history to reduce future model context. This works even when auto compaction is disabled. |
| `/goal [condition]` | Set a completion condition and start working immediately. Rho explicitly tells the agent that this is a goal-setting action, then evaluates the transcript after each turn and continues until the condition is met. Connection errors and other incomplete runs are retried automatically while the goal remains active. If only steps requiring user authority remain, the goal pauses as blocked and reports those steps. Run `/goal` for status, `/goal resume` after completing blocked steps, or `/goal clear` to cancel. |
| `/skills` | Show available workspace skills and insert a `/skill:<name>` command for one. Running the inserted command loads the skill through the skill tool before the model responds. Add text after the command to include extra instructions in the same turn. |
| `/hooks` | Reload [lifecycle hooks](/hooks) and show what each one will run: the resolved argv, working directory, timeout, and environment. Also names any project hooks file ignored because the workspace is not trusted. |
| `/agents` | Reload agent definitions and browse their descriptions, sources, runtime (`rho` or `claude-cli`), model policies, reasoning levels, tools (Rho capabilities or Claude tool names), Claude config inheritance, prompt policies, and prompt previews. Select a reserved internal agent to configure its model. |
| `/diff` | Show local Git status plus staged and unstaged worktree patches without invoking the model. |
| `/doctor` | Check provider authentication, the selected model, config and session writability, model caches, clipboard image helpers, rtk, Herdr integration, and Claude Code binary/auth health without displaying secrets. |
| `/limits` | Fetch and show the usage windows reported by connected OAuth providers. Codex OAuth, Kimi Code OAuth, and xAI OAuth are supported when logged in; absent windows are omitted. Also shows the last Claude Code rate-limit observation from a prior `claude-cli` run (window, status, reset, age) without percentages or a probe. |
| `/export [path]` | Export the current session to a self-contained HTML transcript. Assistant Markdown, including inline `$...$` or `\(...\)` and display `$$...$$` or `\[...\]` LaTeX math, is rendered in the exported file. |
| `/title <name>` | Rename the current session. Replaces any auto-generated title. |
| `/help` | Show keyboard shortcuts and composer controls in a searchable overlay. |
| `/exit` | Quit the TUI. |

Custom prompt templates loaded from prompt files or [`[prompt_templates]`](/configuration#prompt-templates) also appear in the command palette. Completing one inserts its prompt into the composer so you can add or edit text before sending.

A single `/` as the first character opens the command palette. Any later `/` characters are treated as normal message text and do not reopen the palette. While a goal is active, the status line shows an `◎ /goal active` indicator with the evaluated turn count and elapsed time. A goal paused for user action shows `◎ /goal blocked`; sending a new message or running `/goal resume` asks the agent to verify the blocked steps before continuing implementation work.

Some commands can replace the message box with a picker. Use `up` and `down` to select, type to filter by case-insensitive regex, press `tab` to autocomplete the filter from the highlighted item, press `enter` to confirm, and press `esc` to cancel. In conversation and internal-agent model pickers, press `ctrl-p` to pin or unpin the highlighted model; pinned models are saved in config and shown first in both picker types. `/config` starts with a short category browser. Its search matches the settings listed inside each category. Press `enter` to open a category and `esc` to return. Press `space` on an on/off setting to toggle it in place. Changes save at once and return to the same category so you can keep adjusting its settings; login workflows close the picker while credentials are entered or authorized.

In supervised mode, a tool that wants to write a file or execute a process opens a dedicated approval prompt in the composer. The prompt opens on the start of the request, names the capability class, and focuses **Deny** by default. Use the arrow keys to choose **Allow once**, **Allow for session (exact request)**, or **Deny**, then press Enter. **Allow for session** remembers only that exact structured capability request for the current session. Long operation details grow with the terminal height; use Page Up and Page Down to inspect every detail page without hiding the choices. Choosing **Deny** rejects that operation without ending the session. Press Escape to deny and cancel the current run. The active `plan` or `supervised` mode appears in the status line; the default `auto` mode stays hidden to avoid clutter.

Type `@` to open a workspace file picker. Keep typing to fuzzy-search paths, use `up` and `down` to select, then press `tab` or `enter` to insert the highlighted path into the message as an `@path` reference. The picker follows `.gitignore`, `.ignore`, and global Git ignore rules while still showing hidden workspace files that are not ignored.

## Login and logout

`/login` opens a readable provider picker first. Providers with multiple methods open a second picker such as **API Key** or **OAuth**; providers with one method continue directly to their login flow. Passing an internal provider name (for example `/login openai`) targets that method directly. Each flow is documented on the [provider page](/authentication-and-models#providers). Credentials for normal providers are stored in the configured credential backend, not in config or transcripts. When the backend is still unset, Rho asks where to store secrets only after you select a normal provider.

Under **Anthropic**, the method picker includes **Claude Code (delegation only)** next to the Anthropic API key method. `/login claude-code` suspends the TUI and hands the terminal to the `claude` binary for `claude auth login --claudeai`. Claude Code owns that sign-in and stores the subscription credential. Rho never sees the token, never writes it to the Rho credential store, and never asks for a Rho store choice on this path. Install the binary first if needed ([installation](/installation#claude-code-binary-optional)).

`/logout` opens a provider picker containing only providers with stored credentials that can be deleted, or targets one directly (for example `/logout openai`). Environment overrides are CI/development hatches and can keep a provider available after logout. `/logout claude-code` asks for explicit confirmation first because it signs out of Claude Code everywhere the `claude` binary is used, not only inside Rho. It does not delete a Rho-stored token.

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

`/model` remains available while an agent run is active. You can browse the picker or select a model directly, but the current run continues using its existing model through all remaining model steps and tool calls. Rho handles the queued model change only after the full agent loop ends, before the next queued message starts. Selecting another model before then replaces the pending choice. If the finished conversation has reusable live context and older history that can be compacted, or if the target model cannot use provider-native context from the current conversation, Rho asks how to continue before switching.

Rho does not treat a newly resumed session as proof that a provider cache is warm. Compaction can still miss a provider cache, so the choice does not claim a fixed cost saving. Compaction is salvage into portable text; it does not make provider-native blocks sendable to an incompatible model. If handoff compaction fails or produces no reduction, Rho keeps the source model active.

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
| `/help` | Open the keyboard shortcuts overlay |
| `@` | Open workspace file path autocomplete |
| `up` / `down` | Re-enter previous prompts, or select a command or file while a picker is open |
| `tab` | Complete the selected command or file path |
| `enter` | Send a prompt, run a selected slash command, or steer after the current assistant turn while a response is running |
| `alt-up` | Pull the most recent queued prompt back into the composer for editing |
| `ctrl-r` | Reset conversation history |
| `pageup` / `pagedown` | Scroll the transcript viewport |
| `ctrl-g` | Open the current composer text in a non-empty `$VISUAL`, else `$EDITOR` |
| `ctrl-end` | Jump the transcript viewport back to the bottom |
| mouse wheel | Scroll the transcript viewport |
| left-click and drag | Select transcript text and copy it on release |
| code block `COPY` | Copy the full code block contents |
| `ctrl-c` | Clear input, then quit if pressed again |

`ctrl-g` opens the current composer text in a non-empty `$VISUAL`, falling back to `$EDITOR` only when `VISUAL` is unset or empty, both while idle and while a response is running. Rho temporarily restores the normal terminal before starting the editor and resumes the TUI after the process exits. The editor receives expanded pasted text rather than any collapsed display marker. Rho removes one conventional final line ending from the edited file when it restores the composer. Set `VISUAL` or `EDITOR` to an executable path or a platform-native command line with arguments. Rho does not pick a default editor; if neither variable is set or non-empty, it warns with `EDITOR is not set`.

Copied text is sent to the terminal clipboard, and Rho briefly shows how many characters were copied. Code block copy buttons are shown in the top-right border and highlight on hover.

When the transcript is scrolled away from the bottom, Rho overlays a right-aligned `↓ jump to bottom  ctrl+end` button on the last transcript row and obscures only the button's own cells. During generation, the spinner is similarly overlaid on the left. At the live bottom, transcript content stops one row above the spinner; while manually scrolled, the complete last row remains visible wherever neither control is drawn. Press `ctrl-end` or click the button to resume following live output.

Use [automation and CLI](/automation-cli) when you want a single answer outside the TUI.
Use [workflows](/workflows) when you need a frozen multi-step graph with durable status, cancellation, and resume. In the interactive TUI, run `/workflow` to browse sources, plans, and runs without leaving the session.
