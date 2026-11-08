# ACP host reference

Parent: [Agent Client Protocol](/integrations/acp).

What `rho acp` sends and accepts over ACP, for people building or debugging a host. For setup and everyday use, see [Agent Client Protocol](/integrations/acp). For T3 Code, see [T3 Code](/integrations/acp/t3-code).

`rho acp` is a client of the [rho daemon](/daemon): it maps each ACP session to a daemon session and relays it. The daemon runs the turns, tools, and logins, so where this page says Rho does something in a session, the daemon does it, as the [protocol reference](/protocol) describes. A daemon that cannot be reached or started fails the request with `internal_error` naming the cause.

## Initialize

Rho speaks ACP protocol versions 1 and 2 (draft) and answers each connection with the highest version at or below the one the host asked for; a request for version 0 gets `1`. The same negotiation applies to `rho acp --t3code`; use an up-to-date T3 Code build as described in [T3 Code](/integrations/acp/t3-code). The rest of this page describes v1. See [Protocol v2](#protocol-v2) for what changes on a v2 connection. The response carries `agentInfo` (v2: `info`) with `name: "rho"`, `title: "Rho"`, and `version` set to what `rho --version` prints.

Every connection must start with `initialize`; Rho picks the protocol version from it and closes a connection that sends anything else first.

stdout is protocol only and logs go to stderr. Do not write other text to stdout, and do not point a logger at stdout while a host is connected.

Rho advertises:

| Capability | Meaning |
| --- | --- |
| `loadSession` | `session/load`. See [Load and replay](#load-and-replay). |
| `promptCapabilities.image`, `.embeddedContext` | Prompts may carry images and embedded resources. `audio` is `false`. |
| `sessionCapabilities.list`, `.delete` | Saved sessions as host history. See [List and delete](#list-and-delete). |
| `sessionCapabilities.resume` | `session/resume`. See [Resume](#resume). |
| `sessionCapabilities.close` | Close a live session. See [Close](#close). |
| `sessionCapabilities.additionalDirectories` | Extra workspace roots. See [Workspace roots](#workspace-roots). |
| `mcpCapabilities.http` | Host MCP servers over Streamable HTTP. `sse` is not advertised. See [MCP servers](#mcp-servers). |
| `auth.logout` | `logout`. See [Logout](#logout). |

Rho reads these host capabilities. It never sends `fs/*` or `terminal/*` requests, whatever the host advertises: tools run in the daemon, on the files on disk and in its own processes.

| Host capability | Effect |
| --- | --- |
| `clientCapabilities.auth.terminal` | Terminal sign-in methods. See [Authentication](#authentication). |
| `clientCapabilities.session.compaction` | `compaction_update` cards. |
| `clientCapabilities.session.notices` | Non-conversation information as `notice`. |
| `clientCapabilities.session.configOptions.boolean` | `fast_mode` as a boolean switch. See [Fast mode](#fast-mode). |
| `clientCapabilities.elicitation.form` | The `questionnaire` tool and MCP elicitation. See [Questions](#questions). |
| `clientCapabilities.elicitation.url` | OAuth sign-in through `authenticate`. See [Authentication](#authentication). |
| `clientInfo.name` `t3-code` | T3 Code's [question form shape](#t3-code-forms). |

## Protocol v2

A host that sends `"protocolVersion": 2` gets the [ACP v2 draft](https://agentclientprotocol.com/protocol/v2/migration). Rho's behavior is the same as on v1 except where v2 changed the protocol:

- **Initialize.** `capabilities.session` advertises the baseline session methods with `prompt.image`, `prompt.embeddedContext`, `mcp.stdio`, `mcp.http`, `delete`, and `additionalDirectories`. Auth methods carry `methodId`. Compaction cards, notices, and boolean config options need no host capability.
- **Prompt lifecycle.** Once the session inserts the prompt's message, Rho sends a `user_message`, then `state_update` `running`, and answers `session/prompt` with the message's `messageId`. A prompt whose message is saved, a model turn or a `/workflow:` start, is accepted once the message is saved, so a later replay gives it the same `messageId`; a command that saves nothing, such as `/compact`, gets an ID that is not replayed. The turn ends with `state_update` `idle` carrying the stop reason, or `stopReason: "error"` with the error when the turn failed after it started. An error answer to `session/prompt` means the message was never inserted, as when the session is busy, unknown, or closing, the command is rejected, or a cancel arrives first; no updates are sent for it. `session/cancel` ends the turn with `idle` and `cancelled`.
- **Resume.** `session/load` is gone. `session/resume` with `replayFrom: {"type": "start"}` replays the conversation as [Load and replay](#load-and-replay) describes, sending each message with empty content (`user_message`, `agent_message`, or `agent_thought`) before its chunks, so a host that still holds the message replaces it instead of appending; without `replayFrom` it replays nothing. Both send `state_update` `idle` before answering. Other replay cursors fail with invalid params.
- **Setup responses.** `session/new` and `session/resume` answer with `configOptions` (`configId`, `groupId`) and `availableCommands` instead of sending the command list afterwards. There is no `modes` field and no `session/set_mode`; use the `mode` config option.
- **Updates.** Tool calls are created by their first `tool_call_update`. The checklist is a `plan_update` with `planId` `rho-todos`. Command input hints are tagged `type: "text"`. `current_mode_update` is not sent; mode changes arrive as `config_option_update`.
- **Diffs.** Each file change is a `changes` entry (`add`, `delete`, or `modify`; a move is a `delete` and an `add`) with a `git_patch` using absolute paths. A path Git would quote gets no patch.
- **Permissions.** The approval's question is the request `title`; the tool call goes in `subject`.
- **Authentication.** `authenticate` and `logout` are `auth/login` and `auth/logout`.

## Authentication

Rho advertises one authentication method per auth profile the daemon lists. Where it runs depends on the host:

| Auth profile | Host advertises `elicitation.url` | Otherwise |
| --- | --- | --- |
| OAuth (Codex, GitHub Copilot, SpaceXAI, Kimi, Meta, OpenRouter) | Agent method, run by `authenticate` | Terminal method, if the host advertises `auth.terminal` |
| API key | Terminal method, if the host advertises `auth.terminal` | Same |

API keys never travel over ACP, since hosts log their messages, so they are entered only in a terminal. A host that can run neither kind gets no methods, and so does every host when the daemon cannot be reached.

### Agent methods

An agent method has no `type`, which is ACP's default, and its ID is the auth profile. `authenticate` starts the profile's login in the daemon, as `rho login <auth-profile>` does: a browser callback where a browser can appear, and device code over SSH or headless. Rho does not open a browser itself. It sends one URL-mode `elicitation/create` scoped to the `authenticate` request (`requestId`). The URL is the authorize URL or, for device code, the verification URL with the code filled in when the provider supplies one. The message names the provider and includes any device code.

When the provider confirms the login, the daemon saves the credentials, Rho sends `elicitation/complete` for that elicitation, and answers `authenticate` with an empty result. A declined or dismissed elicitation, or `$/cancel_request` for `authenticate`, cancels the login in the daemon, and the request fails with `request_cancelled` (`-32800`). A failed login answers `internal_error` with a reason that does not include the credential. If the host rejects the elicitation, the reason names the `rho login <auth-profile>` command to run instead. A host that does not advertise `elicitation.url` gets `invalid_params` naming that command.

One login runs at a time per connection: a new `authenticate` cancels one still waiting, which then fails with `request_cancelled`, so a host that leaves sign-in without `$/cancel_request` does not keep a login running. The daemon runs one login per auth profile, so a login another client already started fails with `invalid_request`, as does a profile whose credential is set in the daemon's environment, such as `OPENAI_API_KEY`.

### Terminal methods

Each terminal method appends `--login <auth-profile>` to the configured agent invocation; for example, T3 Code launches `rho acp --t3code --login codex` in its sign-in terminal. This signs in through the daemon, as `rho login` does, instead of serving ACP, and needs no working model. API-key input is hidden, including pasted keys. Browser and device-code methods use the same flows as `rho login <provider>`. Cancelled and failed terminal logins exit nonzero, so a zero exit means the provider confirmed the login or the key was saved.

Rho advertises no method for a sign-in whose approval the provider never reports, such as Ollama Cloud's device key, and `rho acp --login` refuses one: sign in with `rho login <auth-profile>` instead.

Credentials stay in the daemon's [credential store](/daemon#credentials), not in ACP messages.

### Credential failures

When a prompt's turn fails because the selected provider has no credentials, the credential store cannot read them, or the provider refuses them, `session/prompt` fails with ACP `auth_required` (`-32000`) instead of an internal error. A refusal covers a revoked API key and an OAuth session whose refresh token expired or was revoked, and applies to model turns and `/compact` alike. The message comes from the daemon and names only the provider, the auth profile, and how to log in; it never contains the credential or the provider's raw response. A turn that fails on its configuration fails with `invalid_params`.

After a successful login, no reconnect is needed, however it signed in. The daemon applies a sign-in to every session it runs from that session's next prompt.

### Logout

`logout` deletes the saved credentials of the auth profile the workspace config selects, as `/logout <provider>` does in the TUI, through the daemon. With no auth profile selected, it fails with `invalid_request` (`-32600`) and deletes nothing. Credentials for other auth profiles are kept. The credentials are deleted from the daemon's store, so every session the daemon runs, whichever client opened it, and other Rho processes that share the store must sign in again; the daemon's sessions fail with `auth_required` from their next prompt. A profile whose credential is set in the daemon's environment cannot be signed out from ACP: `logout` fails with `invalid_request` naming the variable. A store that cannot be opened or written fails with `internal_error`. The result is empty.

`logout` and agent-method `authenticate` run one at a time. A `logout` cancels an `authenticate` still waiting, which fails with `request_cancelled` (`-32800`), so that login cannot save after the logout. A later `authenticate` likewise cancels a `logout` still waiting. A terminal login runs in its own process and is not affected.

## Sessions

ACP sessions are daemon sessions: they run in the daemon, persist under `~/.rho/sessions`, and appear in `rho sessions list` and `rho -R`. See [Sessions](/sessions). `session/new` opens one in the daemon; `session/load` and `session/resume` attach to a saved or running one, reopening it in the daemon when it is not loaded there.

Any number of clients can attach the same session: another `rho acp` process, [`rho attach`](/automation-cli#attaching-to-a-daemon-session), or another [daemon client](/protocol). A prompt another client sends shows up in the host as a user message followed by its turn. A `session/prompt` while another client's turn runs fails with `invalid_request`.

### List and delete

Listing and deleting saved history do not start a model or require provider credentials.

- `session/list` returns the daemon's [session list](/protocol#listing-renaming-and-deleting-sessions) newest first: those of `cwd` when the request names one, else those of every workspace. It lists the same sessions `/resume` offers, so sessions with nothing saved yet are left out, and so are sessions whose workspace directory no longer exists. Each entry has the session's `cwd`, its saved [workspace roots](#workspace-roots) as `additionalDirectories` (leaving out roots that no longer exist), its title (or, when untitled, its first message on one line), and `updatedAt`. Pages hold 100 sessions; pass `nextCursor` back as `cursor` for the next page. Every listed `sessionId` opens with `session/load`.
- `session/delete` deletes a saved session through the daemon, the same way `rho sessions rm` does, including its delegated-agent runs. A session open on this connection is detached first, after its prompt is cancelled and its turn has ended, since hosts such as Zed may delete a thread from history without closing it. Deleting is idempotent: an ID with nothing saved, whether never created or already deleted, succeeds. The daemon refuses a session another Rho process, such as a TUI, has open (`invalid_request`, `-32600`), and one with a turn or background work still running.

### Load and replay

`session/load` replays the session as message chunks and tool calls/updates marked with `_meta.isReplay: true`. Hosts that keep their own transcript can skip these notifications; hosts that rebuild the transcript from the replay can ignore the marker.

- The replay is the saved conversation, then the turn under way, if any, whose later events arrive live.
- Tool cards retain their saved names, arguments, success/failure status, output, and tool-owned images. Background result cards replay too.
- Each saved message replays as one chunk carrying a `messageId` (see [Message IDs](#message-ids)). IDs are derived from where the message was saved, so loading the same session again replays the same IDs.
- Each replayed call gets an ACP identity of its own, so provider IDs reused across turns do not collapse distinct cards.
- A call without a saved result, such as an interrupted one, is marked failed rather than left running.
- The checklist, if any, closes the replay as a `plan`. When the history holds no title update, the session's stored title follows as `session_info_update`.

Nothing beyond the replay is sent while `session/load` is in flight, because hosts such as T3 Code discard live updates for a session until its load completes. After the response, Rho sends the command list, then puts any approvals and questions still open in the session to the host.

A session open in another Rho process that is not a daemon client, such as a terminal TUI or a foreground `rho run`, fails to load with `invalid_request` (`-32600`) naming the session, because only one process writes a session file at a time. Loading a session already open on this connection detaches it and attaches it again.

The session keeps the mode, model, reasoning, and fast mode a client last chose for it, across reloads and daemon restarts; one no client changed takes the configured settings again when the daemon reopens it. `/agent:` sessions rebind to their agent's prompt, tools, and model, and [per-call model](#per-call-model) children restore their saved selection.

### Resume

`session/resume` opens a session the way `session/load` does, for hosts that keep their own transcript. It takes the same `cwd`, `additionalDirectories`, and `mcpServers`, follows the same rules, answers with the same modes and config options, and sends the command list after its response. It replays nothing: no `session/update` reaches the host before the response. A session from `session/new` that was never prompted has no file yet; the daemon keeps it loaded, so it resumes until the daemon stops.

### Close

Hosts such as Zed and T3 Code send `session/close` when a thread closes. Rho cancels the prompt this host is running in the session, as `session/cancel` would, then detaches. The session stays in the daemon, with its MCP servers, background processes, and delegated agents, until the daemon [unloads it](/daemon#unloading-idle-sessions); `session/load` can open it again, unless it closed before its first prompt saved it, which the daemon then discards. Closing an ID with no session open on this connection succeeds and does nothing.

When the host closes its stdio, or the `rho acp` process gets `SIGINT`, `SIGTERM`, or `SIGHUP`, every session it has open is closed the same way; a second signal exits without waiting for the daemon. A process killed with `SIGKILL`, or one that crashes, closes nothing: a prompt it was running keeps its turn running in the daemon, where another client can cancel it.

## Session updates

During a session, Rho sends:

| Update | When and what |
| --- | --- |
| `tool_call`, `tool_call_update` | A call starts, then progress and a final `completed` or `failed`. A call to a tool that does not exist arrives as a single `failed` `tool_call`. Calls still running when a prompt ends, such as after `session/cancel`, get a final `failed` before the prompt response, since ACP v1 has no cancelled tool status. |
| `usage_update` | Context fill (`used`) and the model's window (`size`, `0` when unknown), at each model step and compaction and when a prompt ends. Also the cumulative `cost`; see [Cost](#cost). |
| `plan` | The full `todo` checklist whenever it differs from the last one the host received. An empty `entries` list means the checklist was cleared, per ACP v1. |
| `config_option_update` | The complete option list when it changes, including after `/agent:<name>` selects a different model or tool set and after either settings API applies a change. Hosts replace their previous options, so controls that no longer apply disappear. |
| `available_commands_update` | The complete command list when it changes. See [Commands](#commands). |
| `current_mode_update` | After plan approval switches the mode. |
| `compaction_update` | Each compaction, automatic or `/compact`, when the host advertises `clientCapabilities.session.compaction`. See [Compaction](#compaction). |
| `session_info_update` | The session's `title` once Rho names it. See [Titles](#titles). |
| `notice` | Information outside the conversation that the daemon reports, when the host advertises `clientCapabilities.session.notices`; other hosts get it as an agent message. |

### Message IDs

Every `agent_message_chunk` and `agent_thought_chunk` carries a `messageId`. Consecutive chunks of one kind share it. A new message, with a new ID, starts at a chunk of the other kind, at each model step, and after a tool call or compaction update between chunks, so text after a tool call is a new message. The provider stream-reset notice is a thought message of its own. Message IDs are the daemon's, and a prompt's `/compact` result is a message of its own. Notices sent as agent messages get IDs Rho mints. See [Load and replay](#load-and-replay) for replayed IDs.

### Tool kinds and locations

A `tool_call`'s `kind` follows what the tool does: `read`, `search`, `edit`, `execute`, or `fetch`. An `apply_patch` call whose every file section deletes a file is `delete`, and one whose every section moves a file is `move`; mixed patches are `edit`. `todo` calls are `think` and `exit_plan_mode` calls are `switch_mode`. Other tools are `other`.

`locations` are absolute paths, resolved against the session's `cwd`. ACP does not say how `line` counts, so Rho sends 0-based rows, as Zed reads them and as Zed's own tools send them: line 1 of a file is `0`. A `read_file` call with an `offset` names that line. When a call finishes, its final `tool_call_update` replaces the locations with what the result points at: each file an edit or `write` changed, at its first changed line (no line for new or emptied files), or the first 20 files a `grep` matched, at each one's first shown match. Zed opens every location it gets, and its own grep sends at most 20. Other results keep the started locations.

### File diffs

Successful `write` and edit-tool calls include ACP `diff` content with an absolute `path` and the complete `oldText` and `newText`. New files omit `oldText`; deletions use empty `newText` and carry `_meta: {"rho/fileDeleted": true}`, which tells them from a file emptied in place. Moves appear as a deletion of the source and a creation of the destination. The normal text result stays alongside the diffs. If a write cannot read the existing file as UTF-8, it keeps the text notice instead of inventing original contents. Tools that supply only a unified patch send that patch as text, not as `newText`.

### Cost

`usage_update` carries the session's cumulative `cost` in `USD`, read from Rho's usage ledger, the same record `/spend` reports.

- The total covers every provider request the session made, including retries, compaction, titles, advisor calls, and the session's direct subagents.
- Each request is priced like `/spend`: the cost the provider reported, or else the cached models.dev price.
- Because the total comes from the ledger, it carries across Rho restarts and `session/load`, and no request is counted twice.
- `cost` is omitted until at least one request has a price, so unpriced and local models never show a made-up `0`. After that, it is the sum of priced requests, as in the TUI statusline; requests on unpriced models add nothing.
- A proxy route without its own catalog price borrows the price of another route for the same model only when this session also used that route, so the total can differ slightly from `/spend`.

### Compaction

A `compaction_update` card opens `in_progress` and ends `completed`, `failed` with the error, or `cancelled`. A card still open when its prompt ends, including one whose prompt fails, closes the same way. Cards carry no `summary`, which ACP reserves for retained summary content; the before and after token counts arrive in `usage_update`. Each compaction gets a new random ID, so hosts that keep their transcript across reconnects never update an earlier card. Hosts that do not advertise compaction see automatic compaction only as the drop in `usage_update`.

### Titles

As in the TUI, Rho asks its title model for a short title after the first turn a session completes, unless the session already has one, and saves it for `session/list`, `/resume`, and `rho sessions`. `session_info_update` goes out once the title model answers, usually after that prompt's response. A failed title sends nothing, and the session stays listed by its first message.

A title that lands after this host closed the session is saved but not sent; `session/load` sends it at the end of its replay. Rho does not send `updatedAt`; `session/list` reports it.

## Config options

`session/new` and `session/load` advertise these `configOptions`, all selects except `fast_mode` on hosts that support boolean options. Hosts change any of them with `session/set_config_option`.

| ID | Category | Values | Present when |
| --- | --- | --- | --- |
| `mode` | `mode` | `bypass`, `auto`, `allow_edits`, `plan`, `supervised` | Always, as the first entry |
| `model` | `model` | `provider/model` | Always |
| `thought_level` | `thought_level` | `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, filtered to the model | The model has configurable reasoning |
| `fast_mode` | `model_config` | `on` (Fast), `off` (Normal); `true`/`false` on boolean hosts | The model supports fast mode |

Every change applies to later prompts in the daemon session and keeps the conversation. Every client attached to the session gets the changed options. None rewrites `config.toml`. The session keeps its settings across reloads and daemon restarts; a setting it can no longer take, such as a model whose credentials are gone, falls back to config. See [Load and replay](#load-and-replay) for `/agent:` sessions.

### Mode

The modes go out twice with the same IDs: as the legacy `modes` field and as the `mode` option. Per ACP, hosts that support config options ignore `modes` and use `mode`; older hosts use `modes`. `session/set_mode` and `session/set_config_option` on `mode` change the same state. A new session starts in the configured mode.

Write grants carry over the same way they do in the TUI. Switching to `auto` fails with invalid params unless a classifier model is configured.

### Model

Options are the TUI picker's catalog list: favorites first, value IDs `provider/model`, and the full `provider/model` ID as the description. Labels stay short so host pickers do not truncate them:

- The catalog name when one is known (`GPT-6 Sol`), else the bare model ID.
- When two rows would share a label, each gets its provider appended (`Grok 4.7 (xai)`).
- When one provider gives several models the same name, those rows show their model IDs instead (`gpt-5.6-sol`, `gpt-5.6-sol-pro`).

Re-selecting the current value, including an off-catalog current model, is a no-op success. A switch uses the TUI's conversation-switch path: it renegotiates reasoning, rebuilds compaction, updates later delegated agents, and appends the model-switch context line when history is not empty. After a switch, `thought_level` follows the new model.

### Reasoning

Value IDs are Rho reasoning IDs, filtered to what the current model advertises. Labels are capitalized for display (`xhigh` shows as `Extra high`). Models without configurable reasoning omit the option, so hosts show no control. Config sets the starting value.

### Fast mode

`fast_mode` is the ACP counterpart of the TUI's `/fast`: on sends later turns on the priority service tier, and off uses the standard tier. Turning it on for a model without fast mode is rejected. Typed `/fast` is not an ACP command.

Its shape depends on the host, because ACP only allows boolean options for clients that advertise them. A host that advertises `clientCapabilities.session.configOptions.boolean` at `initialize`, such as Zed, gets a `boolean` option and sets it with `"type": "boolean"` and a boolean `value`. Other hosts, such as T3 Code, get a select with values `on` (named Fast) and `off` (named Normal). A value in the other shape is rejected with invalid params.

A model switch keeps the setting, as in the TUI. Switching to a model without fast mode drops `fast_mode` from the list and the tier has no effect there; switching back advertises it again with the same value. A new session starts from `fast_mode` in `config.toml`.

### Settings during a prompt

As ACP allows, hosts can send `session/set_mode` and `session/set_config_option` at any time, including while a prompt runs. A running prompt keeps the mode, model, reasoning, and fast mode it started with.

- A change sent while a turn is running, this host's or another client's, or while a cancelled turn winds down, waits in the daemon for that turn to end, then applies and responds. It takes effect from the next prompt.
- Changes apply in the order they arrive, and a `session/prompt` sent after `session/cancel` queues behind them. Hosts that interrupt to send a message with new settings, such as T3 Code, keep those settings this way.
- A change the daemon took applies even if the host closes the session before the response.

## Permissions

When a tool needs approval, Rho sends `session/request_permission`. The host's choices map to allow once, allow for this session, and reject.

The daemon puts each approval and question to every client attached to the session. The first answer wins, and Rho withdraws the request from the other hosts with `$/cancel_request`. A request waits until someone answers it or the turn ends; a host that disconnects leaves it open, and a host that loads the session later is asked again. A host that cancels or fails the request leaves it open too, for another client or `session/cancel`.

### Leaving plan mode

In `plan` mode the model gets the `exit_plan_mode` tool, as in the TUI. It sends the finished plan as a `session/request_permission` on the tool's card, with kind `switch_mode` and the plan as content. Every ACP host answers permission prompts, so this does not need [questions](#questions).

The options approve into a mode, with mode IDs as option IDs: `allow_edits`, `supervised`, `bypass`, and `auto` when a classifier model is configured. A final `keep_planning` option rejects. Hosts draw this prompt their own way: Zed lists every option, and T3 Code picks the first option of each kind it shows, `supervised` (allow once), `allow_edits` (allow always), or `keep_planning`.

An approval applies when the proposing turn completes. Rho switches the session to the chosen mode, sends `current_mode_update` and the changed `config_option_update`, and runs the implementation turn in the same `session/prompt`, which ends after that turn.

- If the proposing turn is cancelled or fails, the approval is dropped with a notice.
- A `session/cancel` that arrives after the turn ends but before the switch drops it too, and the prompt ends `cancelled`.

To give feedback instead, choose keep planning and reply in the next message.

## Questions

When the host advertises `clientCapabilities.elicitation.form`, as Zed and T3 Code do, the model gets the `questionnaire` tool and MCP servers may [ask through Rho](/integrations/mcp#questions-from-a-server). Each question goes to the host as a form-mode `elicitation/create` scoped to the session and to the asking tool call. Sessions opened by hosts that do not advertise it get no `questionnaire` tool, and an MCP server's question to them is declined, so nothing waits on a form the host cannot draw. Such a host leaves a question in a session another client opened to that client.

Each question becomes one property keyed by its ID:

| Question | Property |
| --- | --- |
| Single choice | `string` with both `enum` and titled `oneOf` |
| Multiple choice | `array` of titled choices |
| Yes/no | `boolean` |
| Free text | `string` |
| Choice that also accepts free text | The choice, plus an optional `string` property `<id>.other`. A typed answer replaces a single choice and joins a multiple choice. |

A short question header becomes the property `title` and the full question its `description`. Pre-selected defaults go out as `default`. Properties go out in ID order, which hosts such as Zed follow, not in the order the questions were asked.

The host's answer maps to the asking tool:

| Host response | Result |
| --- | --- |
| `accept` | The answers. Content that does not answer the questions, such as a missing required answer or an unknown choice, counts as `decline`. |
| `decline` | The tool call fails with "the user declined to answer"; the model continues. |
| `cancel` | The tool call fails with "the user dismissed the question without answering"; the model continues. |

`session/cancel` withdraws an open form with `$/cancel_request` and ends the prompt `cancelled`.

### T3 Code forms

T3 Code (`clientInfo.name` `t3-code`) reads only a top-level `enum`, accepts free text on every question, and requires an answer to each one. Rho sends it one `string` property per choice question, with the choice labels as `enum` and no `<id>.other` companion.

- Answers may name a choice by label or value; an exact label wins.
- A multiple choice is typed as a list separated by commas, or, when a choice label contains commas, by another character that no label contains (semicolons first). The form's description names the separator. If the labels leave no such character, the question takes one typed choice.
- Free text for a question that accepts only its listed choices counts as `decline`.

## Workspace roots

Hosts with several workspace roots, such as a Zed multi-root workspace, send the extra roots with `session/new`, `session/load`, and `session/resume` as `additionalDirectories`. `cwd` stays the primary workspace. Each extra root joins the session as an [added directory](/tools-workspace#additional-workspace-directories), the same as `--add-dir`: reads there count as workspace reads in every permission mode, and its `AGENTS.md` chain loads. Directories passed to `rho acp --add-dir` apply to every session on top of the host's roots.

- Each entry must be an absolute path to an existing directory. A request with an empty, relative, missing, or non-directory entry fails with invalid params and no session starts.
- As with `--add-dir`, a root inside `cwd` or inside another root adds nothing, since it is already in scope.
- A `null` list counts as empty, but an entry that is not a string, such as `null`, fails the request with invalid params, following the ACP schema.

The roots are saved with the session, so `rho -R` restores them, and `session/list` reports them as `additionalDirectories`, which include directories passed to `rho acp --add-dir` when the session ran. They stay with the session across reloads and daemon restarts. A `session/load` or `session/resume` that reopens the session in the daemon adds the roots it sends to the saved ones that still exist, and one that sends none keeps them. A session the daemon still has loaded keeps the roots it opened with and ignores those the request sends.

## Background results

ACP v1 gives an agent no way to start a turn, so Rho keeps a prompt open instead of waking an idle session. Background work runs in the daemon, so it outlives the host closing the session.

- A result that arrives while a prompt is running reaches the model at its next step.
- If the model ends its turn while a delegated agent Rho runs, or a workflow or background process started during that prompt, is still running, the prompt stays open and the model continues in it once that work finishes. Those agents hold the prompt open whichever prompt started them. Processes left over from earlier prompts do not; their results arrive at the next prompt.
- `/workflow:` ends its prompt without waiting. The run's result reaches the model at the next prompt, which does not wait for a run still going.
- Cancelling stops the wait, not the work. The next prompt waits for the agents Rho runs and delivers their results; processes report in it too.
- A `session/prompt` sent right after `session/cancel` waits for the cancelled prompt to finish instead of failing as busy. Hosts that steer by cancelling and resending, such as T3 Code, can interrupt a waiting prompt without losing its agents.

Each result that reaches the model is also sent as a finished tool call card, the same notification card the TUI shows: a process's outcome, command, run time, and output tail, a delegated agent's result, or a workflow's outcome. A failed process or agent marks its card `failed`. The card's `content` is markdown, with process output in a code fence. Its `rawOutput` is the same text as a plain string with the output unfenced, for hosts such as T3 Code that show tool output as is. These cards are saved with the session, so `session/load` replays them. A background process has no live card while it runs; its card appears once its result is delivered.

## Stop reasons

`session/prompt` ends with one of the ACP v1 stop reasons:

| Stop reason | When |
| --- | --- |
| `end_turn` | The model finished its answer. Commands that run no model turn, such as `/compact` and `/workflow:`, also end this way. |
| `max_tokens` | The model's final turn hit the provider's output-token limit, so the answer may be cut off. Anthropic `max_tokens` and `model_context_window_exceeded`, Chat Completions `length`, and Gemini `MAX_TOKENS` report it. |
| `refusal` | The provider refused or filtered the model's final turn. Anthropic `refusal` and Chat Completions `content_filter` report it. |
| `max_turn_requests` | The run used its model-step budget. |
| `cancelled` | The host cancelled the prompt or closed the session. |

Only the turn that ends the prompt counts. If a cut-off turn still produced complete tool calls, Rho runs the tools and the prompt continues. Any partial answer streams as `agent_message_chunk` before the stop reason arrives.

After `refusal`, the refused prompt and the reply leave the session: the next prompt does not send them to the model, and `session/load` does not replay them. The session file keeps them as an abandoned branch, which the TUI's `/tree` shows. A refused turn that ran tools, or that took steering or background results, is kept instead, because its effects already happened. This matches Zed, which removes a refused prompt from its thread unless a tool finished during that turn.

Some providers report a limit or a refusal as a failure instead, and the prompt fails with a JSON-RPC error just as it does in the TUI. These include an OpenAI Responses `response.incomplete` (for example `max_output_tokens` or `content_filter`), a Gemini `SAFETY` or other blocking finish, and an Anthropic refusal with no output.

## Commands

Right after each `session/new`, `session/load`, and `session/resume` response, Rho sends an `available_commands_update`. Hosts show it as their command menu and send a picked command back as `/name args` prompt text. The commands are listed in [Commands](/integrations/acp#commands).

The daemon checks both menus, commands and skills, at the end of each turn and after an agent rebind. Skills, prompt templates, workflows, and agent definitions added between prompts or by a turn's tools appear on the next check. Unchanged menus are not resent, and nothing watches the filesystem while the session is idle.

- `/compact` runs no model turn. It shows as a compaction card on hosts that advertise compaction, then the compacted context's fill in `usage_update`, then what it did as an agent message.
- `/agent:<name>` is accepted only by a session with no conversation yet; a later one fails with invalid params. A host that starts a Rho child session itself can bind it to an agent this way.
- `/workflow:<name>` ends the prompt without a model turn and adds the run ID to context. Inputs are one JSON object; omitted inputs use their defaults.
- Other TUI commands fail with invalid params naming the command. Text that names no command, such as `/usr/bin` or an unknown template, reaches the model unchanged.

### Per-call model

For a one-call model selection, a fresh session also accepts `/agent:<name> --model <reference> -- <task>`. The `--` delimiter is required; the task may span paragraphs and keep attached images. `/agent:<name> -- <task>` keeps option-like task text literal. The selection follows the same catalog, alias, auth, and permission rules as [`agent.model`](/subagents#per-call-model-selection).

- The child identity and effective selection are saved together before the first model-selection update reaches the host, even before the first provider request.
- `session/load` restores the effective provider, model, and auth profile, including any later model switch, without changing the agent definition or its identity. Adding optional credentials later does not replace a saved keyless profile.
- A later user-driven model switch supersedes the saved override immediately, including a reload before the next prompt.
- A switch that violates the child's definition constraints, such as required fast-mode support, is rejected before changing the session.

### Skills

When the active agent has the `skill` tool, the update also lists one `$<name>` command per discovered [skill](/skills), using the skill's description. Rho leaves out names a host would read as amounts, such as `$20k`.

A turn that mentions known skills as whitespace-delimited `$<name>` tokens loads each of them before the first model request, following the TUI's [mention rule](/skills#load-a-skill-yourself). A prompt that starts with `/$<name>`, as a client that prefixes every picked command with `/` sends it, loads that skill first. Any currency symbol works as the sigil. Every mentioned skill loads once, in mention order, as one batch of `skill` tool calls, and the host sees one `tool_call` update per skill.

## MCP servers

Rho starts MCP servers from its own config, plus the `mcpServers` the host sends with `session/new`, `session/load`, or `session/resume`. Host servers belong to that session only, and are never saved, since their environment and headers may hold secrets. A load or resume connects them only when it reopens the session in the daemon; a session the daemon still has loaded keeps the servers it opened with. A host server replaces a config server with the same name.

- stdio servers start with the host's command, arguments, and environment.
- HTTP servers connect over Streamable HTTP with the host's headers. The URL must be HTTPS or a loopback address.
- SSE servers are skipped with a warning.
- Host servers cannot request sampling.
- Servers may ask the user questions only when the host advertises form elicitation; see [Questions](#questions).
- When an entry names an environment variable or header twice, the last value wins.
- An entry with an invalid name, empty command, or unsafe URL is left out instead of connected. It still claims its name: a config server with that name stays disconnected for the session rather than connecting with the config's endpoint and credentials.

`rho acp` is not itself an MCP server. See [Model Context Protocol](/integrations/mcp).
