# Upgrading to Rho 3

Rho 3 runs agent work in a per-user background process, the [daemon](/daemon). The interactive TUI, `rho acp`, `rho run --detach`, `rho attach`, and `rho mcp serve` are clients of it. A turn keeps running when the client that started it closes, and any client can follow it again later.

This page lists what changes when you move from Rho 2.x, what was removed, and what scripts, editor setups, and Rust code must change. The [changelog](/changelog) lists every change.

| Crate | From | To |
| --- | --- | --- |
| `rho-coding-agent` (the `rho` CLI) | 2.x | 3.0.0 |
| `rho-sdk` | 5.x | 6.0.0 |
| `rho-providers` | 2.x | 3.0.0 |
| `rho-agent-tools` | 1.x | 2.0.0 |
| `rho-protocol` | new | 0.1.0 |

`rho-protocol` stays at 0.x until the protocol is declared stable. While it is 0.x, a client and a daemon work together only when their protocol versions are equal ([Versions](/daemon#versions)).

## Before you upgrade

- **Finish or cancel workflow runs.** Rho 3 stores a root task's files under `nodes/s0.<definition>/`. An attempt that a 2.x process left uncertain under `nodes/<definition>/` is not found by Rho 3's recovery.
- **Keep a copy of any session you might reopen in 2.x.** Rho 3 opens 2.x sessions. Once Rho 3 saves to one, Rho 2.x can no longer read it. See [Sessions](#sessions).
- **Your config is backed up for you.** The first Rho 3 start copies each config file it rewrites to `<name>.2x.bak`. See [Configuration](#configuration).

## Install and update

Install Rho 3 as described in [Installation](/installation). Nothing else is required. The daemon has no service of its own: the first client that needs it starts it, on Linux, macOS, and Windows. It runs one per user and Rho home (`~/.rho`, or `$RHO_HOME`). On Linux and macOS clients reach it over a private Unix socket. On Windows they use a named pipe that only your user can open ([Socket and permissions](/daemon#socket-and-permissions)).

| Command | Use |
| --- | --- |
| `rho daemon status` | Show whether a daemon runs, with its version, jobs, and socket. |
| `rho daemon stop` | Stop it. Turns under way are recorded `interrupted`. |
| `rho daemon restart` | Restart it, for example on the version you just installed. |
| `rho doctor` | Check the daemon, its socket or pipe, and its credential store without starting it ([Checking the daemon](/daemon#checking-the-daemon)). |

After an update, a running daemon replaces itself with the new executable once no work would be cut short ([Upgrading in place](/daemon#upgrading-in-place)). An installer that puts the new version at another path, such as mise or Homebrew, leaves the daemon on the old version. Run `rho daemon restart` from the new version in that case.

Only `rho mcp serve --http` needs a service manager, because it listens before any client connects. The unit examples are in [Running as a service](/daemon#running-as-a-service). [MCP server](/mcp-server) covers serving Rho's sessions over MCP, and [Model Context Protocol](/integrations/mcp) covers the MCP servers Rho uses.

## What changes when you use Rho

### The interactive TUI

`rho` opens a session in the daemon and follows it, on every platform. This is the only interactive TUI.

- **Quitting detaches.** A turn under way keeps running in the daemon. `rho --resume <id>` reattaches and shows what happened while the TUI was closed.
- **`RHO_TUI_DAEMON` is ignored.** Rho 3 development builds used it to choose between the daemon and a session in the TUI's own process. That in-process mode is gone. Rho 2.x always ran the session in the TUI's process.
- **`--config` is refused.** Every session of a daemon uses the config in its Rho home. To run on another config, set `RHO_HOME` to another directory. See [`--config` and `RHO_HOME`](#config-and-rho-home).
- **`/jobs`** lists every job the daemon runs, across projects. From it you can answer prompts, send messages, cancel, start, and attach ([interactive TUI](/interactive-tui)).
- **Commands run with your terminal's environment.** Each prompt carries the environment of the client that sent it, so `PATH`, `mise` shims, and exported variables apply as they do in your shell ([Environment](/daemon#environment)).
- **`behavior.keep_cache_warm`** now runs in the daemon, whether or not a TUI is attached ([Daemon-backed TUI](/daemon#daemon-backed-tui)).

### `rho run` and `rho attach`

`rho run` without `--detach` still runs in your terminal's process and accepts `--config`. `rho run --detach` starts the run in the daemon, prints the session id, and exits ([Running in the daemon](/automation-cli#running-in-the-daemon)).

`rho attach <ID>` follows a daemon session's turn until it ends. It takes a session id or a job id, including a subagent's job, and answers approvals and questions from the terminal ([Attaching to a daemon session](/automation-cli#attaching-to-a-daemon-session)). The 2.x `rho attach`, which read a subagent's run files, is gone. In the TUI, `/attach` and the activity rail still open a subagent's transcript.

### Editors and `rho acp`

`rho acp` is a daemon client. Closing the editor, or the thread, detaches, and the session keeps running in the daemon. Another editor, `rho attach`, or the TUI can follow it.

- **Remove model and config flags from the editor's agent command.** `rho acp` refuses `--provider`, `--model`, `--config`, `--auth`, `--reasoning`, `--permission-mode`, and `--save`. Choose the model and mode in the editor.
- **Tools run on the daemon's machine.** Rho no longer asks the editor to read or write files or to run terminals (ACP `fs/*` and `terminal/*`).
- **Tool-call locations use 1-based lines on ACP v1 too.** Rho 3 uses `agent-client-protocol` 5.0, and a location's `line` is 1-based on v1 and v2 ([ACP host reference](/integrations/acp/reference)).

See [Agent Client Protocol](/integrations/acp) for setup and sign-in.

### `--config` and `RHO_HOME` {#config-and-rho-home}

A daemon reads the config in its own Rho home, so a per-command config file cannot apply to a daemon session. These commands refuse `--config`:

| Command | Error starts with |
| --- | --- |
| `rho` (the TUI) | `--config is not supported in a daemon session` |
| `rho run --detach` | `rho run --detach does not take --config` |
| `rho login` | `rho login does not take --config` |
| `rho acp` | `rho acp does not take --config` |

Set `RHO_HOME` instead. Each Rho home gets its own daemon, config, sessions, and credentials.

### What the model sees

Each user message the model receives now starts with a line giving the time it was sent, in the local time of the machine the session runs on, such as `Thursday, Nov 12, 2026, 12:39 AM (UTC-5)`. The time is written into the message when it is created and saved with the session, so a resumed session sends the same bytes and keeps the provider's prompt cache. The transcript shows the message as you typed it. Assistant output is sent back unchanged.

### One process per session

Only one Rho process writes a session at a time. While the daemon has a session loaded, Rho 2.x cannot open it, and the daemon cannot reopen a session that Rho 2.x has open ([One process per session](/sessions#one-process-per-session)). The daemon lets go of a session after it has been idle for 60 seconds ([Unloading idle sessions](/daemon#unloading-idle-sessions)).

## Configuration

Rho 3 splits the 2.x `config.toml`: TUI settings move to `tui.toml` and `clients/tui.toml`, and plaintext web-search keys move to the credential store. Every Rho start checks for 2.x keys, rewrites the files, prints what moved, and keeps a `.2x.bak` copy of each file it rewrote. [Upgrading from Rho 2.x](/configuration#upgrading-from-rho-2-x) lists every key. A key that neither version knows is still a load error.

Other configuration changes:

- **`edit_tool = "hashline"`**, under `[behavior]` or in a custom provider table, becomes `"str_replace"`. The `hashline` edit format and its `edit` tool are removed. An agent definition that lists `hashline` keeps its edit capability ([Edit formats](/tools-workspace/edit-format)).
- **Hook `tools` lists.** `edit` and `computer` are no longer tool names. A `hooks.toml` that lists `tools = ["edit"]` fails to load with `tool 'edit' is not in the canonical tool list`. List `str_replace` and `apply_patch` instead ([Canonical tool names](/hooks#canonical-tool-names)).
- **Hook payloads** use schema version 3 for every event, workflow lifecycle events included. When RTK rewrites a shell command, `capability.requested_command` carries the command the model asked for ([Hook protocol](/hooks/protocol)). A handler that rejects unknown versions must accept 3.
- **`[internal_agents.goal-judge]`** is ignored, and you can delete it. `goal-judge` is no longer a reserved agent id.
- **The MCP `sampling` key** still parses, so configs that set it load, but it does nothing ([sampling](/integrations/mcp#completions-for-a-server)).
- **Antigravity agent definitions** must list `read_file` and `write_file`. The `view_file` and `create_file` aliases no longer parse.
- **Computer use files.** Rho no longer reads `~/.rho/computer-use.toml` or `~/.rho/computer-use-sessions/`, and you can delete them. A Cua Driver that `/computer setup` installed stays on disk. Remove it yourself if you no longer need it.

## Sessions

- **Sessions from Rho 1.11 and later** (session formats 4 and 5) open in Rho 3. The first save rewrites the session's model history in the Rho 3 snapshot format (`rho-sdk` snapshot schema 3), which Rho 2.x cannot read.
- **Sessions from Rho 0.4.0 through 1.10.x** (formats 1 to 3, written June 21 to July 21, 2026) are refused, including any later Rho continued in place. Opening one fails with `this session was created by rho before 1.11 (session format v2) and can't be opened in rho 3; open it with rho 2.x`. Rho 2.x still opens them ([Compaction and transcript history](/sessions#compaction-and-transcript-history)).
- **Calls to the removed `edit` tool** in saved sessions replay as generic tool cards.

## Removed in Rho 3

| Removed | Instead |
| --- | --- |
| The in-process TUI and `RHO_TUI_DAEMON` | The daemon-backed TUI, the only TUI. |
| `/goal` and its goal loop | State the outcome you want in the prompt. `/goal` was a legacy feature that kept prompting until an evaluator judged a condition met. Current models do not need it. |
| Computer use: the `computer` tool, `/computer`, `rho computer` | None. Daemon sessions may not have your desktop. |
| MCP sampling (`sampling/createMessage`) | None. Rho declares no `sampling` capability and answers the request as method not found. |
| ACP client file and terminal delegation (`fs/*`, `terminal/*`) | Tools run in the daemon on its own filesystem and processes. |
| The file-reading `rho attach` | `rho attach <ID>` on the daemon, and `/attach` in the TUI. |
| `--config` in daemon sessions, and so per-config daemons | `RHO_HOME`. |
| Session formats 1 to 3 | Rho 2.x opens them. |
| The `hashline` edit format and its `edit` tool | `str_replace` (the default) or `apply_patch`. |
| Workflow records from before 2.15, and the compatibility fields in workflow JSON | See [Scripts and integrations](#scripts-and-integrations). |
| `claude_session_id` and `claude_model` in `result.json` | `runtime_session_id` and `runtime_model`. |
| The hidden `rho __classifier_eval` and `rho __compaction_eval` commands | None. They were development harnesses. |

## Scripts and integrations

- **Subagent `result.json`.** A delegated run writes `runtime_session_id` and `runtime_model`. Read the `runtime` field to know what the id means. Rho 3 still reads the old names from 2.x run directories ([Attachment and artifacts](/subagents/attachment-and-artifacts)).
- **Workflow records from before 2.15.** Status, run, resume, and cancel fail with `could not open workflow run <id>: it was saved by Rho before 2.15; inspect it with Rho 2.x or delete it from the /workflow hub`. `rho workflow list` prints a `warning:` line on stderr for each one, and the `/workflow` hub shows them as unsupported rows, which you can delete there.
- **Workflow JSON.** Plan JSON has `program` and `program_digest`, not `graph` and `graph_digest`. Status JSON reports nodes, outputs, and outcomes through scopes, not flat fields. The CLI error code `legacy_record` is now `unsupported_record`. Root task ids are qualified like every other task, for example `s0.review`, in JSONL events, hook labels, and status ([Workflows](/workflows)).
- **Daemon clients.** Frontends in other languages speak the [daemon protocol](/protocol), published as the [`rho-protocol`](https://crates.io/crates/rho-protocol) crate and as `crates/rho-protocol/schema/protocol.json`.

## Rust crates

### `rho-sdk` 6.0

Host-facing enums:

- `QuestionKind`, `DefaultSelection`, `HostInputSource`, `CompactorCancellationMode`, `HookSessionFailureKind`, and `ScriptedToolOutcome` are `#[non_exhaustive]`. Add a wildcard arm. Decision adapters return `InvalidRequest::UnsupportedQuestionKind` for a kind they do not support. `Message` and `ContentBlock` stay exhaustive on purpose.

Providers:

- Implement one `send_turn_stream(request, ProviderTurnContext)`. It replaces `send_turn_stream`, `send_turn_stream_with_options`, and `send_turn_stream_steerable`. Build the context with `ProviderTurnContext::new(events).with_options(..).with_steering(..)`.
- `native_compact(request, options)` replaces `native_compact_with_options`.
- `provider_steering_channel()` returns a `ProviderSteeringSender`.
- Render history with `Message::wire()`, set `replay` on the `ProviderContextBlock`s you produce, and send `ToolResult.images` in your converters.

History and results:

- `Message` gains `CompactionSummary { trigger, text }`, `ModelContext(String)`, `BoundaryInput(Vec<ContentBlock>)`, and `ReasoningShift { from, to }`. Stop parsing summary or model-context text out of user messages. 5.x history converts when it loads.
- `ToolResult` carries `status: ToolResultStatus` and `images` instead of `ok: bool`. Use `ToolResult::succeeded`, `failed`, `with_images`, and `is_failure()`. The image-supplement API (`ToolImageSupplement`) is removed.
- `ToolCall.execution` replaces the async-call marker blocks. `ModelEvent::ToolCallAccepted`, `ModelEvent::Stopped(ModelStopReason)`, and `ProviderContextBlock.replay` (`ExactModel`, `ProviderApi`, or `Never`) replace the stop-reason and provider-note markers.
- `UserInput::stamped(MessageTime)` writes the [send-time line](#what-the-model-sees) into a user message.
- Session snapshots use schema 3. A 5.x SDK rejects them as an unsupported schema.

Events and errors:

- `ToolCompletion::Completed(ToolOutput)` replaces `Success` and `CompletedFailure`.
- `RunEvent::ToolStarted { execution, .. }` replaces `ToolDetached`.
- `RunEvent::StepStarted` carries a `context_estimate` instead of `estimated_context_tokens`.
- Handle `SteeringRetraction::Delivered`, a steer already sent to the provider mid-turn.
- `ToolProgress::new(message).with_metadata(..)`, read with `message()` and `metadata()`.
- `Error::InvalidConfiguration`, `Persistence`, and `Interrupted` are `{ message, source }`, and `Error::source()` returns the cause.
- `CompactionState::from_accounting` is removed. Use `from_parts(..)` with the `with_*` setters.

Builders and hooks:

- `HostQuestion` and `HostChoice` use `with_description`, `with_header`, `with_help`, `with_default_value`, and `with_default_selection`. Their getters drop the `_text`, `_ref`, and `_mode` suffixes. `ToolHostCall` uses `with_id` and `id()`.
- `AfterToolUsePayload` has public `capability` and `process` fields. `HookCapability::ExecuteProcess` has a `requested_command` field, and `HOOK_SCHEMA_VERSION` is 3.

### `rho-providers` 3.0

- `ProviderId`, `ProviderRuntime`, `ModelSelectionError`, and `CatalogSdkAdapter` are `#[non_exhaustive]`. Use `ProviderRuntime::default_api_base()` instead of matching runtimes for an API base.
- `ModelMetadata` is `#[non_exhaustive]` and carries `image_input` and `reasoning_control`. Build it from `Default` and set fields. `models_dev::image_input()` and the separate reasoning-control lookup are removed.
- `run_codex_oauth_flow`, `run_openrouter_oauth_flow`, and `run_xai_oauth_flow` are removed. Use the `start_*_browser_login` and `complete_*_browser_login` pairs.
- `start_interactive_login` takes availability explicitly. `start_ollama_device_login()` no longer takes `open_browser`. `InteractiveUserAction` is removed, and `LoginPrompt` is the only way to present a login.
- `InteractiveLoginCompletion::Unconfirmed` carries an `UnconfirmedAuthentication`. Call `.accept(store)` to save it. It replaces `accept_unconfirmed_login`.

### `rho-agent-tools` 2.0

- `EditFormat` is `StrReplace` (the default) or `ApplyPatch`. `EditFormat::Hashline` and the hashline module are removed.
- `Bash::new()` and `PowerShell::new()` take no arguments. Enable RTK with `ShellToolOptions::rtk`. The `rtk` module is private.
- Tool cards (`ToolCard`, `ToolBody`) moved from `rho_agent_tools::tool_card` to `rho_protocol::display`. `ToolBody` gains `Code` and `Search`, and the search-match fields moved from `ToolCard` into `ToolBody::Search`. `ToolBody` stays exhaustive.

### `rho-protocol` 0.1

New. It is the daemon's JSON-RPC protocol and has no dependency on `rho-sdk`. See the [protocol reference](/protocol).
