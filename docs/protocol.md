# Daemon protocol

Frontends drive the [daemon](/daemon) with rho's protocol: JSON-RPC 2.0 over the daemon's Unix socket. This page is the reference for client authors. Rust clients can use the [`rho-protocol`](https://crates.io/crates/rho-protocol) crate, whose `Client` handles framing, the handshake, and notifications. Clients in other languages can generate their types from the protocol's JSON Schema, [`crates/rho-protocol/schema/protocol.json`](https://github.com/matthewyjiang/rho/blob/main/crates/rho-protocol/schema/protocol.json), which lists every method's params and result, every notification, and every error code, keyed by its code.

## Connection

Connect to the daemon's `daemon.sock` (see [Socket and permissions](/daemon#socket-and-permissions)). Every message is one JSON-RPC 2.0 object on one line, UTF-8, ending with `\n`. A line may be at most 16 MiB; a longer one ends the connection with an `InvalidRequest` error. Request ids are integers or strings, and clients send no notifications.

The first request must be `rho/hello`. Any other request before it gets `HandshakeRequired`. The daemon accepts the handshake when the client's protocol version is [compatible](#versions-and-compatibility) with its own, and otherwise answers `VersionMismatch`, whose `data` names both versions and which side has to change: `fix` is `restart_daemon` when the daemon is the older side, and `update_client` when the client is. With `restart_daemon`, `in_progress` counts what restarting the daemon would interrupt at that moment: jobs queued, running, or waiting for input, logins under way, and sessions with work of their own. A client can offer the restart with that count, and can replace a daemon that reports 0 without asking, as rho's own frontends do ([Versions](/daemon#versions)). A refused connection stays open for another `rho/hello`. The `data` of `VersionMismatch` only ever gains optional fields, so a client of any version can read it.

After the handshake the daemon runs a connection's requests concurrently and answers each one when it finishes, so responses can arrive out of order; pair them with their requests by id. Send a request that depends on another only after that one's response arrives.

When the client stops sending, the daemon answers every request it has already read, then closes the connection. When the daemon stops or restarts, it first sends `rho/session/detached` for every session the client follows, then answers the requests it has read and closes. A restarting daemon keeps its socket, so a client that connects again waits until the restarted daemon answers its `rho/hello`. A client that has stopped reading while the daemon closes its connection is dropped once a message has waited 1 second.

## Methods

| Method | Params | Result |
| --- | --- | --- |
| `rho/hello` | `protocol` (`{major, minor}`), `client` (`{name, version}`) | The daemon's `protocol`, `daemon` (`{version, pid}`), and `capabilities`. |
| `rho/daemon/status` | | `pid`, `version`, `protocol`, `started_at`, `uptime_secs`, `jobs` (jobs queued, running, or waiting for input), `socket`, `credential_store`: the [credential store](#credential-store) the daemon keeps provider credentials in, and `upgrade`, present once the daemon's executable was replaced: the replacement's `version` and `waiting_for`, how many things the [upgrade](/daemon#upgrading-in-place) waits for: jobs in progress, logins under way, and sessions with work of their own. |
| `rho/daemon/stop` | | `{}`; the daemon then shuts down as [described](/daemon#commands). |
| `rho/daemon/restart` | | `{}`; the daemon then [re-executes in place](/daemon#upgrading-in-place), interrupting the turns under way. |
| `rho/session/new` | `cwd`, `environment`, `agent`?, `setup`?, `parent`? | Opens a new session whose workspace is `cwd`, an absolute path to an existing directory, and returns its `session` id and `options` ([below](#changing-a-session-s-settings)). `environment` is the client's environment variables, an object of strings, which the session's commands run with ([below](#the-client-s-environment)). The session runs as `agent`, the id of one of the workspace's agents, as `rho --agent` takes it, or the default agent; one the workspace does not define gets `SessionOpenFailed`. `setup` holds the client's other choices for it ([below](#a-session-s-setup)). `parent` names the session whose turn starts this one, as an agent's `sessions` tool does: the new session's job records the job writing that session as its `parent`, and its turns wait for a slot in the shared budget as subagents do ([sessions other sessions start](/daemon#sessions-other-sessions-start)). A session no job of the daemon writes records no parent, though the new session still shares the budget. |
| `rho/session/prompt` | `session`, `content`, optional `timeout_ms`, `environment` | Starts a turn of `content`, a non-empty list of [content blocks](#content-blocks), whose commands and those of later turns run with `environment`, and returns `{}` once the turn began: it saved its prompt, or, for `/compact`, its compaction started, or, for a session another session started, it has to wait for a slot in the [shared budget](/daemon#sessions-other-sessions-start). A session runs one turn at a time; a prompt while a turn is under way gets `SessionBusy`. A prompt that arrives while [settings changes](#changing-a-session-s-settings) wait to apply starts its turn once they have, so the turn runs with them. With `timeout_ms`, the daemon cancels a turn still running that many milliseconds after it took the prompt, and the turn ends `timed_out`. |
| `rho/session/cancel` | `session` | Cancels the turn under way, if any. The turn ends `cancelled`, or `interrupted` when the daemon is stopping; one already past its `timeout_ms` ends `timed_out`. |
| `rho/session/steer` | `session`, `text` | Adds `text` to the running turn's next model request and returns its `id` once the turn accepted it. The steer applies at that request as a `user_message` whose `steer` is `id`; a turn that ends first drops it, so a client that never saw it apply can send it again. Gets `NotSteerable` when no model request is taking input, or the turn ended before it accepted the input. |
| `rho/session/unsteer` | `session`, `id` | Withdraws steer `id` before it applies and returns its `outcome`: `withdrawn` when it was removed before it reached the conversation, `delivered` when the provider already took it (it still applies), `applied` when it is already in the conversation, or `not_found` when the running turn never accepted that id, as for another turn's steer or one a turn dropped when it ended. Gets `NotSteerable` when no model request is taking input. |
| `rho/session/attach` | `session`, `after`?, `environment`, `setup`? | Follows the session's events ([below](#following-a-session)), reopening an unloaded session with its commands in `environment` and with `setup` ([below](#a-session-s-setup)). Returns its `stream`, its `workspace`, the `replay` that follows (`tail` or `reset`) and how many events it holds (`replay_events`), its job `state`, the input requests open now (`inputs`), its `title`, once it has one, and the session's `options` ([below](#changing-a-session-s-settings)). |
| `rho/session/observe` | `session`, `after`? | Follows the session's events as `rho/session/attach` does, for a client that only reads it, and returns the same response. It never loads a session: one the daemon has loaded is followed as an attach follows it, and a saved one it has not loaded stays unloaded. The daemon replays that one as its file keeps it, with how its last turn ended, without starting its MCP servers, OpenAPI sources, or hooks, then detaches the client with `not_loaded`; with no MCP server connected, that replay titles MCP tool calls by their names alone. It carries no `environment` or `setup`, since it opens nothing that would use them. A daemon from before this method answers `MethodNotFound`. |
| `rho/session/detach` | `session` | Stops following the session; no event of it arrives after the response. |
| `rho/session/answer` | `session`, `request`, `answer` | Answers one of the turn's open input requests ([below](#answering-a-turn-s-requests)) and returns `{}` once the daemon has handed the answer to the turn. |
| `rho/session/configure` | `session`, `change` | Changes one of a loaded session's settings for its later turns and returns its `options` ([below](#changing-a-session-s-settings)). |
| `rho/session/mcp` | `session` | Reads a loaded session's MCP server inventory without waiting for its active turn ([below](#session-mcp-inventory)). |
| `rho/session/hold` | `session`, `request` | Stops an open input request's timeout, so it waits for an answer ([below](#timeouts)), and returns `{}`. |
| `rho/session/list` | `cwd`?, `cursor`?, `limit`? | One page of the saved sessions, newest first: `sessions` and, when more follow, `next_cursor` ([below](#listing-renaming-and-deleting-sessions)). |
| `rho/session/rename` | `session`, `title` | Saves the session's title and returns `{}`; clients attached to it receive `title_changed`. |
| `rho/session/delete` | `session` | Deletes the saved session and the sessions it started, unloading it first, and returns `{}` ([below](#listing-renaming-and-deleting-sessions)). |
| `rho/job/list` | | `jobs`: every job the daemon knows, including those earlier daemons ran but not those of sessions it [discarded unsaved](/daemon#unloading-idle-sessions), oldest first. Each has its `id`, `kind` (`session`, `subagent`, or `workflow`), `state`, `workspace`, and timestamps; the `session` it writes, absent for a workflow and for a subagent on another runtime; the `parent` job that started it, for a subagent, a workflow, or a session another session started; the `workflow_run` it drives, for a workflow; the `error` when it failed; and, usually while it is `queued`, its `queue` place: the `budget` it waits on, that budget's `limit`, and its `position`, 1 for the next to start. |
| `rho/auth/key` | `auth`, `key` | Checks the API key with the provider of auth profile `auth`, such as `openai-api-key`, and saves it ([below](#signing-in-and-out)). Returns `{}`, or, when the provider could not be asked, `unchecked` with its `reason` and a `message`. |
| `rho/auth/logout` | `auth` | Deletes auth profile `auth`'s saved credentials ([below](#signing-in-and-out)) and returns `{}`. |
| `rho/auth/list` | | `auths`: every auth profile a client can sign in to, the TUI's `/login` picker's and ACP's auth methods, each with its `auth` id, `provider`, a `label` for a picker, its `kind` (`api_key` or `login`), and its `status` (`signed_in` or `signed_out`). |
| `rho/auth/login` | `auth`, `flow`? | Starts auth profile `auth`'s interactive login in the daemon and returns its `login` id; its prompt and its end follow as notifications ([below](#logging-in)). `flow` is `browser`, the default, or `device_code`. |
| `rho/auth/login_cancel` | `login` | Asks the login to stop, which then ends `cancelled`, and returns `{}`; a login the provider already approved still saves and ends `succeeded`. |
| `rho/auth/choose_store` | `store` | Checks that credential store `store` (`os` or `file`) takes a temporary secret, saves it as the daemon's `behavior.credential_store`, and returns the new `credential_store` ([below](#credential-store)). |

A prompt whose turn ends before it began returns an error instead of a result:

- `TurnEnded`, whose `data` is the turn's [outcome](#how-a-turn-ends): `cancelled` when a cancel arrived before the turn began, `timed_out` when its `timeout_ms` passed first, or `failed` with the reason, such as unreadable provider credentials or a slash command the daemon does not run. Nothing is saved and the model is not called, except that a cancel or timeout that arrives while `/agent:<name>` rebinds the session waits for the rebind: the session stays rebound to the agent, and only the prompt is not saved. A turn that stopped unexpectedly fails with `the turn stopped unexpectedly; see the daemon log`.
- `DaemonStopping` when the daemon starts stopping while the prompt waits for its turn to begin.

A prompt that starts with a slash command resolves as it does in [`rho run`](/automation-cli#sessions-commands-and-questions): prompt templates, MCP prompts, `/init`, `/agent:<name>`, and `$skill` mentions work, and so do `/compact` and `/workflow:<name>`, which run no model turn:

- `/compact [instructions]` compacts the session's context. Clients get the command as a `user_message`, a `compaction` event that starts, then one that completes, followed by `usage` with the compacted context's fill, then what the compaction did as an `agent_message_chunk`, and the turn ends `answered` with the same `text`. A compaction that fails or is cancelled ends `failed` or `cancelled` instead, with no agent message. Neither message is saved: a later reload replays the compaction's own saved row in their place.
- `/workflow:<name> [inputs JSON]` starts the workspace workflow in the background. Clients get the prompt as a `user_message` and the start notice, with the run id, as an `agent_message_chunk`, both saved, and the turn ends `answered` with the notice as `text`.
- `/agent:<name> [--model <reference> --] <task>` rebinds a session that has no conversation yet to that agent, then runs `task` as its turn; clients get the agent's options as `options_changed` before the turn's output.

The TUI's own commands, such as `/model`, do not run in the daemon, and the turn fails as a `configuration` error.

What a session runs, and what its job states mean, is described in [Sessions and jobs](/daemon#sessions-and-jobs).

### Session MCP inventory

`rho/session/mcp` returns a point-in-time `SessionMcpInventory` for the named loaded session, including servers supplied in its setup. It neither loads the session nor changes its settings. An unloaded session gets `SessionNotLoaded`; an unknown id gets `SessionNotFound`.

- `config_path`: the configuration file the daemon opened the session with.
- `mode`: `native`, `unsupported_agent`, or `tools_disabled`.
- `servers`: configured servers, including disabled and failed entries. Each has its qualified `identity`, `enabled`, startup `status` (`connected`, `connecting`, `disabled`, `invalid_config`, `failed`, `timed_out`, or `not_loaded`), exported `tool_count`, and human-readable `detail`.

Details include transport, startup errors, exported tools, tools added or withdrawn since startup, the last failed keepalive ping, current MCP prompt commands, and resource counts. Treat `detail` as presentation text, not a machine-readable contract. `status` describes startup; subsequent reachability is reported in the details. Reissue the request for a fresh snapshot; no inventory subscription is created. It reads shared maintenance handles rather than locking the runtime, so a running model turn or a queued configure does not delay it.

### Content blocks

A prompt's `content`, and a `user_message` event's, is a list of blocks, each with a `type`:

| Block | Fields | What the model sees |
| --- | --- | --- |
| `text` | `text` | The text. |
| `image` | `mime_type`, `data` (standard base64) | The image: a PNG, JPEG, GIF, or WebP. Each image may be at most 32 MiB, the limit pasted images have, but over the socket the whole request must fit in one 16 MiB line, which leaves about 12 MiB of image after base64; a longer line is an `InvalidRequest`. |
| `resource` | `uri`, `text` | A text resource embedded whole, such as an attached file: `text` in a code fence labelled with `uri`. |
| `resource_link` | `uri`, `name`, optional `description` | A resource named but not included: `description`, else `name`, in a code fence labelled with `uri`. |

A prompt with no blocks gets `InvalidParams`, as does one with an image whose `data` is not base64, is over the limit, is not one of those formats, or is not the `mime_type` it declares; the error names the block by its position, from 0. A block of a type the daemon does not know reaches the model as a fenced placeholder. A `user_message` holds the message as saved, so a resource the prompt carried appears as its fenced `text` block, live and on replay.

### A session's setup

`rho/session/new` and `rho/session/attach` take an optional `setup`: the choices `rho acp` takes per session and `rho` takes as flags. The daemon applies it when the request opens the session: a new session, or an attach that reopens a saved one. A session the daemon has loaded keeps what it opened with, as it keeps its environment. Every field is optional:

| Field | What the session gets |
| --- | --- |
| `additional_directories` | Directories outside the workspace its tools may use, as `--add-dir` adds them: absolute paths to existing directories. Any other entry gets `InvalidParams`, naming it, and the session does not open. |
| `mcp_servers` | MCP servers the client provides, connected next to those in config; one replaces a config server of the same name. Each has a `transport`: `stdio`, with `name`, `command`, and optional `args` (a list of strings) and `env`, or `http`, with `name`, `url`, and optional `headers`; `env` and `headers` are objects of strings, by name. The daemon connects plain HTTP only to this machine and never lets these servers sample the model. An entry it cannot connect is left out, and still replaces the config server of its name; when a name repeats, the last entry wins. |
| `without` | Parts of the session left out: `tools`, `system_prompt`, or `subagents`, as `--no-tools`, `--no-system-prompt`, and `--no-subagents` leave them out, or `questions`, for a client that cannot show questions: the session then has no questionnaire tool and declines an MCP server's question. A part this daemon does not know gets `InvalidParams`. |
| `child_threads` | Who runs the children its `agent` tool starts: `daemon`, the default, as subagents of the session, or `host`, as the client's host's own threads, which `rho acp --t3code` asks for. With `host`, the session's `agent` and `agents` tools delegate through the `delegate_task`, `task_status`, and `task_cancel` tools of an MCP server named `t3-code` in `mcp_servers`; while one of them is missing, every `agent` call fails rather than run the child in the daemon. |

The session keeps its directories and the parts it leaves out, other than `questions`, in its file, so they outlast the daemon unloading it. A prompt or attach that reopens a saved session opens it with the directories it saved that still exist and the parts it saved as left out, plus those its own `setup` names: a later client can narrow a session further, never widen it. MCP servers, `questions`, and `child_threads` belong to the client, since a server's `env` and `headers` may hold secrets and `host` child threads go through the client's MCP server: the daemon never saves them, so a reopened session has only those of the attach that reopens it, and none when a prompt reopens it, since a prompt carries no setup. A saved directory that no longer exists is dropped, and the daemon logs a warning naming it. A session `rho run` or `rho acp` saved keeps what their `--no-*` flags left out the same way; one the TUI started leaves nothing out.

That guarantee holds across the daemon's reopens, so it covers `rho acp` sessions, which run in the daemon. The TUI, which opens a session file in its own process, keeps what a session saved but does not save its own `--no-*` flags; it becomes a daemon client in [#1667](https://github.com/matthewyjiang/rho/issues/1667).

## Following a session

After `rho/session/attach`, the daemon sends the session's events as `rho/session/event` notifications, each with the `session`, a `seq`, and the `event`. The attach response comes first. Each event's `type` is one of:

| Event | What it carries |
| --- | --- |
| `state` | The job's new `state`. An ended turn's `turn_ended` comes first. |
| `user_message` | A user message, whole: its `message` id and `content`. A steer that just applied, crossing into the conversation for the turn's next model request, carries the `steer` id `rho/session/steer` returned. |
| `steering_delivered` | The provider already took steer `id` mid-request: it can no longer be withdrawn, and still applies, as a `user_message` carrying `id`, at the turn's next model request. |
| `agent_message_chunk`, `thought_chunk` | More `text` of the agent's reply or reasoning, under a `message` id. |
| `tool_call` | Opens a tool call, or updates the call with the same `id`: its title, kind, status, input (`raw_input`), content, and locations. Once it finishes, its content starts with the text the model got from it, if any. Absent fields are unchanged. Its `display`, when present, replaces the card Rho's TUI draws for the call ([below](#tool-cards)). |
| `plan` | The agent's plan, replacing any earlier one. |
| `usage` | How full the model's context is: `used_tokens`, and `window_tokens` when the model's window is known. Once a request in the session had a price, `cost_usd_micros` holds what the session has cost so far, across daemon restarts, in millionths of a US dollar. A finished compaction reports one too. |
| `title_changed` | The session's new `title`: the generated one, a few seconds after the first answered turn, `rho run --detach` runs included, or one `rho/session/rename` saved. |
| `compaction` | A context compaction began or ended, under its `id`. Its `status` has a `type`: `started`, then exactly one of `completed`, `cancelled`, or `failed` with an `error`. |
| `response_discarded` | The provider's response broke off and is retried. What it streamed cannot be taken back: keep it, or mark it superseded; the retried response starts new messages. |
| `card` | A titled card shown outside the conversation, such as a background result: its body as `markdown`, as `plain` text for hosts that show text as is, and its `tone`: `neutral`, `accent`, `success`, `warning`, or `error`. |
| `notice` | Something to tell the user that is not part of the conversation. |
| `input_requested` | The turn waits for a person: the `request`, with its `id`, what it `ask`s ([below](#answering-a-turn-s-requests)), the `tool_call` that asks, if one does, and its `timeout`, if it has one ([below](#timeouts)). |
| `input_resolved` | The request `id` closed, with its `resolution`: `answered`, `withdrawn`, or `timed_out`. |
| `input_held` | The request `id`'s timeout stopped; it waits for an answer. |
| `turn_ended` | The turn ended, with its `outcome` ([below](#how-a-turn-ends)). |
| `options_changed` | The session's `options` changed ([below](#changing-a-session-s-settings)), all of them, replacing the last. |

Each loaded session has one stream of events, numbered by `seq` from 1. A client keeps the `stream` and the highest `seq` it has handled since its attach response as its cursor, and passes them as `after` when it attaches again, for example after the TUI is closed and reopened. A session the daemon [unloaded](/daemon#unloading-idle-sessions) and reopened since has a new stream. The response's `replay` says what follows:

- `tail`: the daemon still buffers every event after the cursor, and sends those, then the live ones. The client continues where it was.
- `reset`: the cursor is from another stream, too old, at 0, or absent. The client drops what it shows and its cursor. The daemon sends the session's saved conversation, then the events since the last turn saved: the last turn's `turn_ended` and `state`, and the turn under way. The saved conversation's events carry `seq` 0, except the last, which carries the `seq` the turn under way began after, so a client that detaches before it has the whole conversation holds no cursor and resets again.

Saved conversations hold no `usage`, `compaction`, `response_discarded`, `steering_delivered`, or `title_changed` events, and their user messages carry no `steer`; those are live only, so a reset replays them only for turns since the last one that saved. The attach response's `title` is the session's current title.

The first `replay_events` events after the response are the replay; the rest are live. The replay ends in the response's `state`, so a client that attached after a turn ended finds how it ended at the replay's end, and one that attached during a turn waits for a live `turn_ended`. `rho attach` follows a session this way.

The daemon buffers up to 16 MiB of a session's recent events for clients that resume. A turn larger than that still resets correctly once it is saved; a client that resets while it is under way gets a notice that its start is missing.

The daemon detaches a client with `rho/session/detached`, after which no event of that session arrives:

- `lagged`: the client fell 4096 events behind, or the daemon unloaded the session, as a delete that fails after it unloaded the session does. A turn never waits for a client; attach again with the cursor to catch up.
- `daemon_stopping`: the daemon is stopping.
- `restarting`: the daemon is re-executing in place, for an upgrade or `rho/daemon/restart`, and closes the connection next. Connect again and attach with the cursor; the restarted daemon answers with a `reset` replay.
- `deleted`: `rho/session/delete` deleted the session.
- `not_loaded`: `rho/session/observe` found the session unloaded, so its replay was the saved conversation and nothing follows. Observe again to read it again.

Attaching or observing a session the connection already follows replaces the earlier attachment.

### Tool cards

A `tool_call`'s `display` is the card Rho's TUI draws for the call, so every frontend can draw the same one. Its types are the `display` module of `rho-protocol`; in the schema, `Presentation` and the types it uses. A `display` is one of:

- `card`: a card with a status (`running`, `ok`, `error`, or `interrupted`), a header, facts under it, and a body of lines, code, search hits, or diff rows.
- `summary_card`: a card drawn collapsed to its facts until expanded.
- `message`: a notification, such as an agent's report.

A call's start and each progress update carry a `card` that replaces the last one, and its result carries its final display. A call that closes `failed` without a `display` was cut off: draw its last card as interrupted. A reset replays the cards a client watching live got, except for a call the model was still writing when its response was aborted, which a reset does not replay. Images a call returned stay in its `content`.

### How a turn ends

A turn that ends sends one `turn_ended` event; the `state` that records the job's end follows, unless the daemon could not record it. A turn the daemon's stop or restart cuts short sends none: the stop first detaches every client with `daemon_stopping`, or `restarting`, and `rho/job/list` shows the job `interrupted`, as it does for a turn whose daemon died. The `outcome` has a `type`:

| Outcome | Job state | What it carries |
| --- | --- | --- |
| `answered` | `completed` | The model's answer whole as `text`, and why it stopped: `end_turn`, `max_tokens` (the provider cut it off), or `refusal`. Show `text` rather than the chunks you gathered when they differ, as after a provider retry restarted the reply. |
| `max_steps` | `completed` | The turn used its model-step limit before the model answered; `text` is its last step's reply, possibly empty. |
| `cancelled` | `cancelled` | A client cancelled the turn. |
| `timed_out` | `timed_out` | The daemon cancelled the turn at its prompt's `timeout_ms`, which it repeats. |
| `failed` | `failed` | The `error`: its `kind` (`authentication`, `provider`, `tool_host`, `configuration`, or `other`) and a `message`. An authentication failure's message names only the provider, the auth, and how to log in, never the provider's own text, which may echo credentials. |

A client that resets finds, after the saved conversation, the `turn_ended` of each turn since the last one that saved to it. Outcomes are not saved with the conversation.

## Answering a turn's requests

A turn that needs a person, for an approval, a plan approval, or questions, opens an input request and sends `input_requested` to every attached client. The job is `waiting_for_input` while any request is open. A request waits for as long as it takes: it is never denied because no client is attached, and only a [timeout](#timeouts) the user configured answers it for them. A client that attaches later finds every open request in the attach response's `inputs`, even when the replay no longer holds the event that opened it; the replay may repeat some, so key requests by `id`. The session's [`input_requested` hooks](/hooks#events) run too, so a user away from every client can be told.

Any connected client may answer with `rho/session/answer`, whether or not it follows the session. The first answer wins and every attached client receives `input_resolved` with `answered`; a later answer, or one to a request the turn withdrew, gets `InputNotOpen`. A turn withdraws its open requests, with `withdrawn`, when it is cancelled or ends, and a plan approval when the tool that asked stops waiting; a turn that stops waiting just as an answer wins, or just as a timeout fires, ignores it. Request ids are unique within the session for as long as the daemon has it loaded.

Each request's `ask` has a `kind`, which decides the answers it takes; any other answer gets `InvalidParams` and leaves the request open:

| `kind` | What it carries | Answers (`answer.kind`) |
| --- | --- | --- |
| `approval` | The tool or part of rho that asks (`source`), the `operation` it would perform (read or write a path, discover instructions, run a process, reach the network, or load a skill), and the policy's `reason`. | `allow_once`, `allow_for_session`, or `deny` with an optional `reason` for the tool. |
| `plan_approval` | The `plan`, as markdown, and the permission `modes` it may be carried out in. | `approve_plan` with one of those `mode`s, or `keep_planning`. |
| `question` | A `title` and its `questions`, each with an `id`, `prompt`, `choices` (`value`, `label`), its `selection` (`one` or `many`), whether it takes the user's own text (`allow_other`), whether it is `required`, and the answer the agent suggests, if any (`default`: the `values` it would send and whether to pre-select it, `style` `selected`, or only focus it, `focused`). | `answers`, mapping question ids to the chosen choices' values or the user's text; `decline`; or `dismiss`. |

### Timeouts

When the user sets [`[questionnaire] timeout_seconds`](/configuration#questionnaire-timeout), a `question` whose agent offered fallback answers comes with a `timeout`: the `deadline_ms` (Unix time in milliseconds) when the daemon answers it, the `answer` it will give (`answers`), and the agent's `reason` for it. Show the user both, with a countdown. Nobody needs to be attached: at the deadline the daemon answers the request itself, and every attached client receives `input_resolved` with `timed_out`. The tool learns the answers came from the fallback, not the user.

Never answer a timeout yourself: an answer sent with `rho/session/answer` counts as the user's own, so the tool would take the fallback as the user's choice. The daemon answers it.

Once the user starts on the request, on their first key, paste, or click in it, call `rho/session/hold`. The request is then held for good, with no timeout: every attached client receives `input_held`, the request waits for an answer like any other, and a client that attaches later finds it in `inputs` without a `timeout`. Holding a request that has no timeout, or one already held, changes nothing. The setting applies to sessions opened after it changes.

## Changing a session's settings

A session's `options` are what a frontend shows as its settings and command menu: ACP's config options and available commands, and the TUI's model picker, reasoning, fast mode, and permission controls. `rho/session/new` and `rho/session/attach` return them, and every attached client receives `options_changed` when they change: after `rho/session/configure`, at a turn's end, which may leave new skills, prompt templates, or MCP prompts behind, after a plan approval switches the permission mode, and after `rho/auth/key`, `rho/auth/logout`, or a finished `rho/auth/login` changes the models a session may switch to. A session in a turn when a credential write lands refreshes at the turn's end.

| Field | What it carries |
| --- | --- |
| `model` | The `current` model, as `{provider, model}`, and the models the session may switch to (`available`), each with a short `label`; the user's favorites first. |
| `reasoning` | The `current` reasoning level and the levels the model takes (`available`): `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`. Absent when the model's reasoning cannot be configured. |
| `fast_mode` | Whether fast mode is `enabled`: faster responses at a higher credit rate. Absent when the model has no fast mode. |
| `permission_mode` | The `current` permission mode and every mode (`available`): `bypass`, `auto`, `allow_edits`, `plan`, `supervised`. |
| `commands` | The commands a prompt may run, in menu order: each `name`, `kind`, `description`, and `input_hint`, what to type after the name, absent when it takes nothing. A `slash` command starts the prompt as `/<name>`; a `skill` is mentioned anywhere as `$<name>`. `/workflow:<name>` commands are listed when the session has the `workflow` tool, and `/agent:<name>` ones for every workspace agent that runs on Rho. |
| `goal` | The session's [goal](#goals), absent when it has none: its `condition`, `phase` (`active` or `blocked`), `evaluated_turns`, `set_at` (Unix seconds), the `last_reason` its evaluator gave, and, while blocked, the `human_steps` left to the user, each an `action` and a `reason`. |

`rho/session/configure` applies one `change`, tagged by `kind`, and answers with the session's `options` as they are now:

| `kind` | Fields | Change |
| --- | --- | --- |
| `model` | `model` (`{provider, model}`) | Switches to one of `model.available`, keeping the reasoning level; a model that cannot keep it is refused. |
| `reasoning` | `level` | Spends one of `reasoning.available` on later turns. |
| `fast_mode` | `enabled` | Turns fast mode on, for a model that has it, or off. |
| `permission_mode` | `mode` | Gates later turns' tool calls with `mode`. |
| `goal` | `action` | Sets, clears, or resumes the session's [goal](#goals). Unlike the settings above, it never waits for a turn. |

A change the session cannot take, such as a model no saved credential serves or fast mode on a model without it, gets `InvalidParams` naming why, and the session keeps its setting. A change that fails as it applies, such as a model switch whose provider could not be built, gets `InternalError`; the session's `options` then report what it runs with, which `options_changed` carries if that changed. A change sent while the session's turn is under way, waiting for input included, waits for the turn to end, then applies and answers, so a frontend can send settings whenever its user picks them. Changes and turns take the session one at a time, in the order the daemon took their requests: a change applies after the turn of every prompt taken before it, and a prompt's turn runs after every change taken before it, so a change sent once the job reports `running` waits for that turn's end. A change the daemon took applies even if its client disconnects before the answer; while the daemon is stopping, one still waiting gets `DaemonStopping` and does not apply. Only a session the daemon has loaded takes a change; any other gets `SessionNotLoaded`, so attach first.

Settings persist with the session: from its first change on, the session keeps its model, reasoning level, fast mode, and permission mode with its file, as they stand at each save, a plan approval's mode switch included. A session the daemon [unloads](/daemon#unloading-idle-sessions) and reopens, or one a later daemon reopens, runs with them, and its `options` say so. A setting it can no longer take, such as a model whose credentials are gone, falls back to the config's.

### Goals

A goal keeps a session working until an evaluator finds its completion condition met, as `/goal` does in the TUI. The daemon owns the loop: after each turn it evaluates the condition against the session's history, continues with another turn while the goal is unmet, retries a turn that failed, and pauses when only the user can do what is left. The whole loop runs as one turn of the session's job, so it goes on with no client attached, and an unloaded session keeps no goal: a session with a goal stays loaded. A goal lasts while the daemon runs, not across a daemon restart. Its progress reaches clients as `notice` events, and its state as `options_changed`.

`rho/session/configure` with `{"kind": "goal", "action": …}` changes it; `action` is tagged by `kind`:

| `kind` | Fields | Change |
| --- | --- | --- |
| `set` | `condition` | Sets the goal and starts its first turn. The condition, trimmed, takes 1 to 4000 characters. |
| `clear` | | Drops the goal at once, cancelling an evaluation or retry wait, but not a model turn already running. |
| `resume` | | Verifies a blocked goal's remaining steps, or continues an active one. |

`set` and `resume` start a turn, so a session already in one gets `SessionBusy`; `resume` without a goal and a `set` condition out of range get `InvalidParams`. `rho/session/cancel` ends the loop with the turn: an active goal is dropped, while a blocked goal whose verification it interrupts keeps its steps for a later `resume`. A prompt sent while the goal is blocked verifies its steps, with the prompt as the user's message.

## Listing, renaming, and deleting sessions

These methods let a frontend show and manage saved sessions without reading session files ([#1670](https://github.com/matthewyjiang/rho/issues/1670), [#1667](https://github.com/matthewyjiang/rho/issues/1667)). They work on any saved session, loaded or not, and reach the same session store as `rho sessions`, `/resume`, and ACP's `session/list`.

`rho/session/list` lists the sessions a person can resume: those with a saved message whose workspace still exists, leaving out `rho run`'s, subagents', and workflow nodes' sessions, as `/resume` does. With `cwd`, an absolute path, it lists only that workspace's sessions, resolving symlinks as `rho/session/new` does, and none when the path does not resolve to a directory; without it, every workspace's. Each entry has:

| Field | Meaning |
| --- | --- |
| `session` | The session id, which `rho/session/attach` reopens. |
| `cwd` | The session's workspace. |
| `added_dirs` | The extra directories the session can reach that still exist; empty when none. |
| `title`? | The session's title, once it has one. |
| `first_user_message`?, `last_user_message`? | The first and last user messages on one line, cut to 80 characters, as `/resume` previews them. |
| `updated_at` | When the session was last saved, in Unix seconds. |

Sessions come newest first. A page holds `limit` sessions, 100 by default and at most 1000, and a larger `limit` gets `InvalidParams` naming the limit. When more sessions follow, `next_cursor` is an opaque string to send back as `cursor` for the next page. The walk never repeats a session, nor skips one when others are deleted between pages; a session saved during the walk moves ahead of the cursor and is missed, so list again from the first page for a fresh view. A `cursor` that is not one a list returned, such as a malformed one, gets `InvalidParams`.

`rho/session/rename` saves `title`, trimmed, as the session's title, as `rho sessions rename` does. A title that is empty, holds a control character such as a newline or an escape, or runs over 500 characters gets `InvalidParams` naming why. A title it saves is never replaced by a generated one. An id nothing is saved under gets `SessionNotFound`. Ids are chosen by clients, so one id can be saved in two workspaces; a rename or delete of such an id gets `InvalidParams`.

`rho/session/delete` deletes the session's files, with those of the subagent and workflow sessions it started, as `rho sessions rm` does. The daemon unloads a loaded session first, and detaches its clients with `deleted`; a session it loaded that has saved nothing yet has no files, so it is unloaded, its clients detached with `deleted`, and the delete returns `{}`. It refuses with `SessionBusy` while the session's turn is under way, waiting for input included, while a settings change waits to apply or applies, while a subagent, workflow run, or background process it started is still running or holding a result, while a run it started has not finished, or while another job writes it. Another process, such as the TUI, holding the session or a session it started gets `SessionInUse`. A loaded session is checked against all of these before the daemon unloads it, so a refused delete leaves it loaded; should the delete still fail after the unload, as on a disk error, its clients are detached with `lagged` and attaching again reopens it. While the delete runs, a request for the session may get `SessionBusy`, or, once the session is unloaded, `SessionNotFound` or `SessionNotLoaded`. An id the daemon has not loaded and nothing is saved under gets `SessionNotFound`. The session's job record stays.

## Signing in and out

`rho/auth/key` and `rho/auth/logout` change the credential store the daemon's sessions read, as `/login` and `/logout` do in the TUI. The daemon moves to the store its config selects when the request arrives and writes that one; a request it refuses writes nothing. Moving stores alone changes no running session's credentials. A change applies to every session the daemon runs, whichever client opened it: a running session uses a new key, or drops a deleted one, from its next prompt, and new sessions use it at once. After a logout, a session's next prompt fails with `TurnEnded`, whose outcome is a `failed` turn of kind `authentication`, until a key is saved again.

`rho/auth/key` first asks the provider's models endpoint about the key and refuses one the provider rejects, with HTTP 400 or 401, with `KeyRefused`, keeping the key saved before; the error's message names the status. A key the provider cannot be asked about is saved anyway, and the result's `unchecked` says why: its `reason` is `no_model_listing` when rho lists no models for the provider, `forbidden` when the endpoint answered 403, as it does for a key that may run models but not list them, or `unreachable` when its endpoint could not be reached or failed with another status, and its `message` says so in words, such as `HTTP 404`. A client reads a `reason` it does not know as another reason the key went unchecked. Surrounding whitespace is ignored, and `auth` must take an API key; OAuth and other sign-ins use `rho/auth/login` ([below](#logging-in)). `rho/auth/logout` also forgets an Ollama login.

A credential set in the daemon's own environment, such as `OPENAI_API_KEY`, wins over a saved one, so these methods, and `rho/auth/login`, refuse its auth profile with `EnvironmentOverride` before writing anything; its `data.variable` names the variable to unset where the daemon starts. The client's environment plays no part.

The key travels only over the daemon's socket, which only its user can open, and the daemon never logs it, nor the params of any request.

`rho/auth/list` reports what the store the daemon's config selects holds now, logins the TUI or ACP saved in their own processes included.

### Logging in

`rho/auth/login` runs an auth profile's interactive login, such as `openai-codex`'s or `github-copilot`'s, in the daemon, which relays it to the client that started it; `rho login` is such a client. The response, `{login}`, comes before anything else of the login, which then arrives on the same connection:

| Notification | Params | When |
| --- | --- | --- |
| `rho/auth/login_prompt` | `login`, `prompt` | The provider's login is ready. `prompt` holds the `url` to open, and for a device-code login the `user_code` to enter there and, when the provider has one, the `url_with_code` that fills it in, with an `instruction` for the user. The daemon opens no browser: the client opens the URL, or shows it. |
| `rho/auth/login_finished` | `login`, `outcome` | The login ended. The `outcome`'s `type` is `succeeded`, once the credential is saved; `failed`, with a `message`; or `cancelled`. A provider that sends no confirmation (Ollama Cloud's device key) succeeds as soon as the daemon saved the login, since its prompt's `instruction` already told the user what to approve. A client reads a `type` it does not know as a login that did not succeed. |

A browser login waits on a callback port on the daemon's machine, so a client on another machine should ask for `device_code`; a provider with no device flow, such as OpenRouter, runs its browser login either way. The credential is saved as `rho/auth/key` saves a key, and every session the daemon runs uses it from its next prompt.

One login per auth profile runs at a time, whichever client started it: another gets `LoginBusy` until it ends. A login ends when the provider approves or refuses it, when `rho/auth/login_cancel` cancels it, or when the connection that started it closes; the last sends no `rho/auth/login_finished`, since nobody is left to read it. Once the provider approved, the daemon saves the credential even if the client leaves. `rho/auth/login_cancel` for a login that already ended gets `LoginNotFound`. An `auth` that takes an API key, or none, gets `InvalidParams`.

### Credential store

`credential_store` in `rho/daemon/status` names the `store` the daemon's environment and config select, which every sign-in writes: `os` (the Secret Service on Linux, the Keychain on macOS) or `file`. `selected_by` says what chose it: `default` when nothing did, `config` for `behavior.credential_store`, or `environment` for `RHO_CREDENTIAL_STORE` in the daemon's environment. `unreachable`, present only when the daemon cannot reach the store, says why, such as a daemon with no session D-Bus for the Secret Service. The daemon checks by looking up an account that does not exist, writing nothing and prompting for nothing, so a locked keyring counts as reachable until a write is refused. A client reads a `store` or `selected_by` it does not know as another value.

The daemon never moves credentials to another store on its own. While `selected_by` is `default`, a client signing in should ask its user which store to use, offering the file store when the OS store is `unreachable`, as `rho login` does, and send the answer with `rho/auth/choose_store`. The daemon first writes and deletes a temporary secret there, refusing a store that does not take it with `CredentialStoreUnavailable`, then saves the choice in its config and keeps credentials there from then on; credentials already saved stay where they are, and the models each session may switch to follow the new store. A store `RHO_CREDENTIAL_STORE` chose cannot be replaced this way, so the daemon refuses with `EnvironmentOverride` naming the variable.

## The client's environment

A session's commands run with its client's environment, not the daemon's ([#1659](https://github.com/matthewyjiang/rho/issues/1659)). `rho/session/new`, `rho/session/prompt`, and `rho/session/attach` each carry `environment`, the client's environment variables as an object of strings; a client leaves out a variable whose name or value is not UTF-8. A new session's commands start with the opening client's, each prompt replaces it with the prompting client's, and an attach uses its own only when it reopens a session the daemon had unloaded. Processes already running keep the environment they started with. MCP servers, hooks, OpenAPI sources, CLI subagents, a Claude Code advisor, and the workflow runs a session starts read it too, as [Daemon](daemon.md#environment) describes. The daemon leaves provider credentials out of commands' environments, though CLI subagents and the advisor get them, never logs the environment, and never saves it, so a session reopened after a restart takes the reopening client's.

## Errors

Errors use the JSON-RPC `code` and `message`. The first five codes are JSON-RPC's own.

| Code | Name | Meaning |
| --- | --- | --- |
| -32700 | `ParseError` | The line was not JSON. Sent with a `null` id. |
| -32600 | `InvalidRequest` | Not a valid JSON-RPC request, a second `rho/hello`, or a line over 16 MiB. |
| -32601 | `MethodNotFound` | The daemon does not implement the method. |
| -32602 | `InvalidParams` | The params do not match the method's, or one is invalid, such as a `cwd` that is not a directory. |
| -32603 | `InternalError` | The daemon failed while handling a valid request. |
| -32000 | `HandshakeRequired` | A request other than `rho/hello` came before the handshake. |
| -32001 | `VersionMismatch` | The protocol versions are incompatible; `data` says which side to change and, when that is the daemon, how much restarting it would interrupt. |
| -32003 | `SessionNotFound` | No session is saved under the id. |
| -32004 | `SessionBusy` | The session already has a turn under way (for a prompt or a delete), a settings change waiting or applying (for a delete), a delete under way, or, for a delete, background work running or a run it started unfinished; or another job, such as a subagent run, writes the saved session a prompt, an attach, or a delete would touch. |
| -32005 | `NotSteerable` | No model request is taking input. |
| -32006 | `SteerRefused` | The running turn refused the steering input, or a request to withdraw a steer. |
| -32007 | `DaemonStopping` | The daemon is stopping, or restarting in place, and starts nothing new. |
| -32008 | `SessionOpenFailed` | The daemon could not open the session: for example, the config is invalid, the workspace does not define the agent a new session asks for, the agent a saved session was saved as, or the one `/agent:<name>` rebound it from, changed or is gone, or the session is a subagent's or workflow node's, which only its parent continues. |
| -32009 | `TurnEnded` | The turn ended before it began; `data` is its outcome. |
| -32011 | `InputNotOpen` | The input request is not open: another client answered it first, it timed out, or the turn withdrew it. |
| -32012 | `SessionInUse` | Another process, such as a TUI or a foreground `rho run`, has the session open, or, for a delete, a session it started, so the daemon cannot reopen or delete it. |
| -32013 | `EnvironmentOverride` | A variable in the daemon's environment holds the auth profile's credential, or `RHO_CREDENTIAL_STORE` chooses its credential store, so saving or deleting one, or choosing another store, would change nothing; `data.variable` names it. |
| -32014 | `KeyRefused` | The provider rejected the API key, and nothing was saved; the message says what failed. |
| -32015 | `CredentialStoreUnavailable` | The credential store could not be opened, read, or written, as when the OS keyring is locked or the daemon has no session D-Bus to reach it. `rho/daemon/status` says why; `rho/auth/choose_store` chooses another store. |
| -32016 | `SessionNotLoaded` | The daemon has not loaded the session, which a settings change needs; attach to it first. |
| -32017 | `LoginBusy` | The daemon is already signing in to the auth profile; wait for that login to finish. |
| -32018 | `LoginNotFound` | No login under way has the id: it ended, or never started. |
| -32019 | `SignInRequired` | The daemon could not open the session because its model's auth profile has no usable credentials: none are saved or set, they could not be read, or the provider refused them. Sign in, then try again. |

## Versions and compatibility

The protocol has its own version, `major.minor`, independent of rho's; `rho/hello` and `rho/daemon/status` report it.

Major 0 is the unstable protocol, which is the current one: a minor may change anything, every change to the wire raises the minor, and a client and daemon work together only when their versions are equal. It stays at 0.x until a frontend outside rho's repository has run against it ([#1776](https://github.com/matthewyjiang/rho/issues/1776)). The rest of this section describes majors from 1 on, where a client and daemon work together when their majors match.

A protocol minor only adds: methods, notifications, error codes, optional fields, and values of open enums. In the schema, one variant of an open enum is marked `x-rho-other` and matches every value the enum does not list, so a client generated from it accepts values a later minor adds; read them as unknown. For an event or content block the catch-all matches any object with a string `type`, so schema validation alone does not reject a malformed known variant: pick the variant by its `type`, then decode it. A client should also ignore fields it does not know, and treat an error code it does not know by its message. Anything else, such as removing, retyping, or constraining a field, making a field required or nullable, or adding a value to a closed enum, comes only with a new major.

The daemon omits an optional field that has no value rather than sending `null`.

CI holds the protocol to this. A test fails when `protocol.json` no longer matches the wire types, and a check compares it with the schema of the last rho release: a change a minor does not allow fails unless the major rises, and any other change to the wire, which excludes descriptions and names, needs the minor to rise. While the major is 0, a second check compares each change with the commit before it, and any change to the wire needs the minor to rise, so two builds that speak the same version speak the same wire.
