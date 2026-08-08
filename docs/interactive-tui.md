# Interactive TUI

Run `rho` in a terminal to start an interactive coding session in the current directory.

```bash
rho
```

The TUI is the main way to use Rho. Ask it to inspect files, explain code, make changes, run commands, or iterate on a task with you. Rho uses the current directory as its [workspace](/tools-workspace). Tool access and command execution follow the workspace and security behavior described in [tools and workspace](/tools-workspace#security-and-workspace-boundaries).

```mermaid
flowchart TD
    start[rho in project root] --> prompt[Type a prompt]
    prompt --> run[Model and tool loop]
    run --> steer[enter steers]
    run --> abort[esc aborts]
    run --> done[Idle again]
    steer --> run
    abort --> done
    done --> prompt
```

## Start a session

Open a project and run Rho from the repository root:

```bash
cd path/to/project
rho
```

Rho streams the assistant response as it works. Tool use appears inline so you can see commands, file reads, and edits as they happen. For persisted history and resume behavior, see [sessions](/sessions).

If you need auth or a model first, use `/login` and `/model`, or follow [getting started](/getting-started).

## Everyday controls

### Send a prompt

Type a request and press `enter` to send it.

```text
summarize this repository
```

```text
add tests for the config parser
```

```text
find where the TUI handles paste events
```

Use a multiline prompt when you need to paste or write a longer request. Type `@` to open a workspace file picker, fuzzy-search paths, then press `tab` or `enter` to insert an `@path` reference. The picker follows `.gitignore`, `.ignore`, and global Git ignore rules while still showing hidden workspace files that are not ignored.

### Interrupt, steer, reset, or quit

- Press `esc` to abort the current response without closing Rho. The provider request and active tool receive the same cancellation signal, partial assistant output remains in the session, and queued prompts are restored to the composer instead of running automatically.
- Press `enter` while Rho is working to steer the run. Rho finishes every tool call from the current assistant turn, adds their results to context, then inserts the steering message before the next model request.
- Press `ctrl-r` to reset the conversation history. The next message starts a new [session](/sessions).
- Press `ctrl-c` to clear the current input line.
- Press `ctrl-c` twice to quit.

### Keyboard shortcuts

Most editing keys work the way they do in a normal terminal input. Run `/help` for a searchable overlay of the same shortcuts.

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

## Commands

Type `/` at the start of the message box to open the command palette. Keep typing to filter commands, use `up` and `down` to select, press `tab` to complete the selected command, and press `enter` to run it. Most built-in slash commands run locally. Commands that start agent work say so below.

A single `/` as the first character opens the command palette. Any later `/` characters are treated as normal message text and do not reopen the palette.

| Command | Action |
| --- | --- |
| `/advisor [on\|off]` | Toggle or set [advisor mode](/configuration/advisor-mode), which gives the agent an `advisor` tool backed by a second model. `/advisor on` without an advisor model opens a picker first; the mode turns on once a model is selected, and `esc` leaves it off. Reasoning for the advisor model is set under `/config` → **Agent behavior** → **Advisor reasoning** when the model supports it. The choice saves to configuration and applies before the next turn. |
| `/login [provider]` | Log in with a provider or the Claude Code runtime. No args opens a picker (Claude Code is under **Anthropic** as **Claude Code (delegation only)**); direct args target a single [provider](/authentication-and-models#providers) or `/login claude-code`. |
| `/logout [provider]` | Delete stored provider credentials, or sign out of Claude Code everywhere with `/logout claude-code` (after confirmation). No args opens a picker; direct args target a single [provider](/authentication-and-models#providers). |
| `/model [provider/model]` | Open a picker for models with available auth, or choose a provider/model and save it to [configuration](/configuration). When switching would drop provider-native context, or when the current model has completed a live turn and older context can be compacted, Rho asks how to continue. Compaction can summarize portable context first; it does not make native blocks sendable to the new model. Press `ctrl-p` in the picker to pin or unpin the highlighted model. |
| `/fast [on\|off]` | Toggle or set the faster priority tier for supported Codex models. Fast mode saves to configuration, appears as `(fast)` after the model name, and uses credits at a higher rate. |
| `/resume [id]` | Resume a saved session by UUID or prefix. No args opens a picker for other sessions in the current workspace. In the picker, press `d` or `Delete` to remove a session after confirmation. If the current model cannot use the session's provider-native context, Rho asks whether to resume with the session model, compact with that model first, or continue on the current model. |
| `/sessions` | Open the session manager for every directory. Sessions group under their directory, current directory first. Press Enter on a session in the current directory to resume it, or on a directory row to narrow the list to that directory. Sessions from other directories remain available to inspect and delete, but must be resumed by starting Rho in their directory. Press `d` on a session to delete it, or on a directory row to delete the reviewed saved sessions in that directory, both after confirmation. The current session is never deleted. |
| `/tree` | Navigate completed turns and compaction states in the current session. Continuing from an older state creates a branch. |
| `/workflow` | Open the workflow list. Start a local workflow or saved plan in the background, watch a run as a dependency graph, or press `d` to delete a plan/run. The run id is appended to chat context and completion is delivered automatically. Reopen `/workflow` and press Enter on a run to watch; use arrows or `hjkl` to move between graph nodes. |
| `/rewind [turn]` | Preview and restore native file-tool changes from a completed turn, then continue from that conversation state on a new branch. This experimental command requires `behavior.experimental_workspace_rewind = true`. It does not reverse shell, Git, process, network, database, or service effects. Conflicting paths stay unchanged. |
| `/config` | Open the [config](/configuration) category browser for models and reasoning, agent behavior, context limits, tools, providers, and updates. |
| `/info` | Show the running Rho version, provider, model, reasoning level, permission mode, advisor mode, and external runtime status (including Claude Code ownership). |
| `/changelog [latest]` | Show release notes for this installed version from the bundled changelog. `/changelog latest` fetches notes for the newest published release. |
| `/compact` | Immediately summarize older conversation history to reduce future model context. This works even when auto compaction is disabled. |
| `/goal [condition]` | Set a completion condition and start working immediately. Rho explicitly tells the agent that this is a goal-setting action, then evaluates the transcript after each turn and continues until the condition is met. Connection errors and other incomplete runs are retried automatically while the goal remains active. If only steps requiring user authority remain, the goal pauses as blocked and reports those steps. Run `/goal` for status, `/goal resume` after completing blocked steps, or `/goal clear` to cancel. |
| `/skills` | Show available workspace skills and insert a `/skill:<name>` command for one. Running the inserted command loads the skill through the skill tool before the model responds. Add text after the command to include extra instructions in the same turn. |
| `/hooks` | Reload [lifecycle hooks](/hooks) and show what each one will run: the resolved argv, working directory, timeout, and environment. Also names any project hooks file ignored because the workspace is not trusted. |
| `/agents` | Reload agent definitions and browse their descriptions, sources, runtime (`rho` or `claude-cli`), model policies, reasoning levels, tools (Rho capabilities or Claude tool names), Claude config inheritance, prompt policies, and prompt previews. Select a reserved internal agent to configure its model. |
| `/diff` | Show local Git status plus staged and unstaged worktree patches without invoking the model. |
| `/doctor` | Check provider authentication, the selected model, config and session writability, model caches, clipboard image helpers, rtk, Herdr integration, and Claude Code binary/auth health without displaying secrets. |
| `/limits` | Fetch and show the usage windows reported by connected OAuth providers. Codex OAuth, Kimi Code OAuth, and xAI OAuth are supported when logged in; absent windows are omitted. Also shows the last Claude Code rate-limit observation from a prior `claude-cli` run (window, status, reset, age) without percentages or a probe. |
| `/export [path]` | Export the current session transcript. Formats: HTML (default), Markdown (`.md`), JSON (`.json`). Omit the path to write a timestamped file under `~/.rho/exports/` (or `$RHO_HOME/exports/`). A directory argument receives that default file name. The path extension selects the format. Existing files are not overwritten; choose a new path. HTML exports render assistant Markdown math (inline `$...$` or `\(...\)`, display `$$...$$` or `\[...\]`) with KaTeX. Live TUI math uses a narrower TXM path; see [Math rendering](/interactive-tui/math). |
| `/new` | Start a new session. Clears the transcript, composer, attachments, and active goal. The next message creates a new session folder. Unavailable while a model turn is running. |
| `/title <name>` | Rename the current session. Replaces any auto-generated title. |
| `/help` | Show keyboard shortcuts and composer controls in a searchable overlay. |
| `/exit` | Quit the TUI. |

Custom prompt templates loaded from prompt files or [`[prompt_templates]`](/configuration#prompt-templates) also appear in the command palette. Completing one inserts its prompt into the composer so you can add or edit text before sending.

### Pickers

Some commands replace the message box with a picker. Use `up` and `down` to select, type to filter by case-insensitive regex, press `tab` to autocomplete the filter from the highlighted item, press `enter` to confirm, and press `esc` to cancel. In conversation and internal-agent model pickers, press `ctrl-p` to pin or unpin the highlighted model; pinned models are saved in config and shown first in both picker types. `/config` starts with a short category browser. Its search matches the settings listed inside each category. Press `enter` to open a category and `esc` to return. Press `space` on an on/off setting to toggle it in place. Changes save at once and return to the same category so you can keep adjusting its settings; login workflows close the picker while credentials are entered or authorized.

## Login and logout

`/login` opens a readable provider picker first. Providers with multiple methods open a second picker such as **API Key** or **OAuth**; providers with one method continue directly to their login flow. Passing an internal provider name (for example `/login openai`) targets that method directly. Each flow is documented on the [provider page](/authentication-and-models#providers). Credentials for normal providers are stored in the configured credential backend, not in config or transcripts. When the backend is still unset, Rho asks where to store secrets only after you select a normal provider.

Under **Anthropic**, the method picker includes **Claude Code (delegation only)** next to the Anthropic API key method. `/login claude-code` suspends the TUI and hands the terminal to the `claude` binary for `claude auth login --claudeai`. Claude Code owns that sign-in and stores the subscription credential. Rho never sees the token, never writes it to the Rho credential store, and never asks for a Rho store choice on this path. Install the binary first if needed ([installation](/installation#claude-code-binary-optional)).

`/logout` opens a provider picker containing only providers with stored credentials that can be deleted, or targets one directly (for example `/logout openai`). Environment overrides are CI/development hatches and can keep a provider available after logout. `/logout claude-code` asks for explicit confirmation first because it signs out of Claude Code everywhere the `claude` binary is used, not only inside Rho. It does not delete a Rho-stored token.

Logging in does not normally switch provider/model. Use `/model` to switch models and providers. If Rho started without usable auth, a successful login selects that provider's default model so the session can run.

## Choose a model

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

Run `/agents` to inspect reserved internal agents. The detail pane shows the effective provider/model and whether it follows the conversation or uses an override. Press Enter on `session-title`, `goal-judge`, or `advisor` to choose a model. Select **Use conversation model** to remove that role's override. Each role resolves its own setting when invoked, so changing one does not affect the others.

The `advisor` role has no conversation-model fallback, so its picker omits **Use conversation model** and its detail pane reads `not selected` until you choose a model. See [advisor mode](/configuration/advisor-mode).

For provider and auth details, see [authentication and models](/authentication-and-models).

## Approvals and status line

In supervised mode, a tool that wants to write a file or execute a process opens a dedicated approval prompt in the composer. The prompt opens on the start of the request, names the capability class, and focuses **Deny** by default. Use the arrow keys to choose **Allow once**, **Allow for session (exact request)**, or **Deny**, then press Enter. **Allow for session** remembers only that exact structured capability request for the current session. Long operation details grow with the terminal height; use Page Up and Page Down to inspect every detail page without hiding the choices. Choosing **Deny** rejects that operation without ending the session. Press Escape to deny and cancel the current run. The active `plan` or `supervised` mode appears in the status line; the default `auto` mode stays hidden to avoid clutter.

While [advisor mode](/configuration/advisor-mode) is on, the status line names the reviewing model, for example `advisor: anthropic/claude-fable-5`. It reads `advisor: no model` when the mode is on but no advisor model is set, which can happen after a hand edit of config; nothing reviews the session in that state. Advisor mode stays out of the status line while it is off. Advice arrives as a normal `advisor` tool card, collapsed past the tool output limit and expandable with `ctrl+o`.

While a goal is active, the status line shows an `◎ /goal active` indicator with the evaluated turn count and elapsed time. A goal paused for user action shows `◎ /goal blocked`; sending a new message or running `/goal resume` asks the agent to verify the blocked steps before continuing implementation work.

## Watch a subagent

Run `rho attach <id>` to watch a subagent reported by the `agent` tool:

```bash
rho attach abc123
```

Attached mode uses a separate read-only TUI. It renders the delegated prompt, reasoning, assistant output, tool activity, usage, and final state, but it has no message box and cannot submit prompts or change the subagent environment. Use Up/Down, Page Up/Page Down, and Home/End to scroll. Press `q`, Escape, or Ctrl-C to detach without stopping the run. For Claude-cli runs, attach also surfaces `claude_session_id` when present so you can open the full Claude transcript with `claude --resume <session-id>`. Under [Herdr](/integrations/herdr), activating a subagent row opens a sibling pane that runs attach for you. See [subagents](/subagents/attachment-and-artifacts) for lifecycle details.

## Attachments

Paste images with `ctrl+v` when a host clipboard helper is available, or drop a filesystem path. Rho accepts PNG, JPEG, GIF, and WebP images and extracts text from common document types into a bounded attachment.

Details: [Attachments](/interactive-tui/attachments).

## Transcript display

The TUI owns the transcript viewport (use its scroll controls, not terminal scrollback). Headings, copy actions, jump-to-bottom, and stale-stream handling are documented separately.

- [Transcript display](/interactive-tui/transcript)
- [Theme](/interactive-tui/theme)
- [Mermaid diagrams](/interactive-tui/mermaid)
- [Math rendering](/interactive-tui/math)

## Related

Use [automation and CLI](/automation-cli) when you want a single answer outside the TUI.
Use [workflows](/workflows) when you need a frozen multi-step graph with durable status, cancellation, and resume. In the interactive TUI, run `/workflow` to browse sources, plans, and runs without leaving the session.
Under [Herdr](/integrations/herdr), Rho reports agent state and can open attach panes. With [RTK](/integrations/rtk) on `PATH`, agent shell commands are rewritten automatically. See [integrations](/integrations).
