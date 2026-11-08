# Daemon protocol

Frontends drive the [daemon](/daemon) with rho's protocol: JSON-RPC 2.0 over the daemon's Unix socket. This page is the reference for client authors. Rust clients can use the [`rho-protocol`](https://crates.io/crates/rho-protocol) crate, whose `Client` handles framing, the handshake, and notifications. Clients in other languages can generate their types from the protocol's JSON Schema, [`crates/rho-protocol/schema/protocol.json`](https://github.com/matthewyjiang/rho/blob/main/crates/rho-protocol/schema/protocol.json), which lists every method's params and result, every notification, and every error code, keyed by its code.

## Connection

Connect to the daemon's `daemon.sock` (see [Socket and permissions](/daemon#socket-and-permissions)). Every message is one JSON-RPC 2.0 object on one line, UTF-8, ending with `\n`. A line may be at most 16 MiB; a longer one ends the connection with an `InvalidRequest` error. Request ids are integers or strings, and clients send no notifications.

The first request must be `rho/hello`. Any other request before it gets `HandshakeRequired`. The daemon accepts the handshake when the client's protocol major matches its own, and otherwise answers `VersionMismatch`, whose `data` names both versions and which side has to change (`restart_daemon` or `update_client`).

After the handshake the daemon runs a connection's requests concurrently and answers each one when it finishes, so responses can arrive out of order; pair them with their requests by id. Send a request that depends on another only after that one's response arrives.

When the client stops sending, the daemon answers every request it has already read, then closes the connection. When the daemon stops, it first sends `rho/session/detached` for every session the client follows, then answers the requests it has read and closes. A client that has stopped reading while the daemon closes its connection is dropped once a message has waited 1 second.

## Methods

| Method | Params | Result |
| --- | --- | --- |
| `rho/hello` | `protocol` (`{major, minor}`), `client` (`{name, version}`) | The daemon's `protocol`, `daemon` (`{version, pid}`), and `capabilities`. |
| `rho/daemon/status` | | `pid`, `version`, `protocol`, `started_at`, `uptime_secs`, `jobs` (jobs queued, running, or waiting for input), `socket`. |
| `rho/daemon/stop` | | `{}`; the daemon then shuts down as [described](/daemon#commands). |
| `rho/session/new` | `cwd`, `environment` | Opens a new session whose workspace is `cwd`, an absolute path to an existing directory, and returns its `session` id. `environment` is the client's environment variables, an object of strings, which the session's commands run with ([below](#the-client-s-environment)). |
| `rho/session/prompt` | `session`, `content`, optional `timeout_ms`, `environment` | Starts a turn of `content`, a non-empty list of [content blocks](#content-blocks), whose commands and those of later turns run with `environment`, and returns `{}` once the turn has saved the prompt. A session runs one turn at a time; a prompt while a turn is under way gets `SessionBusy`. With `timeout_ms`, the daemon cancels a turn still running that many milliseconds after it took the prompt, and the turn ends `timed_out`. |
| `rho/session/cancel` | `session` | Cancels the turn under way, if any. The turn ends `cancelled`, or `interrupted` when the daemon is stopping; one already past its `timeout_ms` ends `timed_out`. |
| `rho/session/steer` | `session`, `text` | Adds `text` to the running turn's next model request. Gets `NotSteerable` when no model request is taking input. |
| `rho/session/attach` | `session`, `after`?, `environment` | Follows the session's events, reopening an unloaded session with its commands in `environment` ([below](#following-a-session)). Returns its `stream`, its `workspace`, the `replay` that follows (`tail` or `reset`) and how many events it holds (`replay_events`), its job `state`, and the input requests open now (`inputs`). |
| `rho/session/detach` | `session` | Stops following the session; no event of it arrives after the response. |
| `rho/session/answer` | `session`, `request`, `answer` | Answers one of the turn's open input requests ([below](#answering-a-turn-s-requests)) and returns `{}` once the daemon has handed the answer to the turn. |
| `rho/session/hold` | `session`, `request` | Stops an open input request's timeout, so it waits for an answer ([below](#timeouts)), and returns `{}`. |
| `rho/job/list` | | `jobs`: every job the daemon knows, including those earlier daemons ran, oldest first. Each has its `id`, `kind` (`session`, `subagent`, or `workflow`), `state`, `workspace`, and timestamps; the `session` it writes, absent for a workflow and for a subagent on another runtime; the `parent` job that started it, for a subagent or workflow; the `workflow_run` it drives, for a workflow; the `error` when it failed; and, usually while it is `queued`, its `queue` place: the `budget` it waits on, that budget's `limit`, and its `position`, 1 for the next to start. |
| `rho/auth/key` | `auth`, `key` | Checks the API key with the provider of auth profile `auth`, such as `openai-api-key`, and saves it ([below](#signing-in-and-out)). Returns `{}`, or, when the provider could not be asked, `unchecked` with its `reason` and a `message`. |
| `rho/auth/logout` | `auth` | Deletes auth profile `auth`'s saved credentials ([below](#signing-in-and-out)) and returns `{}`. |

A prompt whose turn ends before it saves the prompt returns an error instead of a result:

- `TurnEnded`, whose `data` is the turn's [outcome](#how-a-turn-ends): `cancelled` when a cancel arrived before the turn began, `timed_out` when its `timeout_ms` passed first, or `failed` with the reason, such as unreadable provider credentials or a slash command the daemon does not run. Nothing is saved and the model is not called. A turn that stopped unexpectedly fails with `the turn stopped unexpectedly; see the daemon log`.
- `DaemonStopping` when the daemon starts stopping while the prompt waits for its turn to save it.

A prompt that starts with a slash command resolves as it does in [`rho run`](/automation-cli#sessions-commands-and-questions): prompt templates, MCP prompts, `/init`, and `$skill` mentions work. `/compact`, `/workflow:<name>`, and `/agent:<name>` do not run in the daemon yet, and the turn fails as a `configuration` error.

What a session runs, and what its job states mean, is described in [Sessions and jobs](/daemon#sessions-and-jobs).

### Content blocks

A prompt's `content`, and a `user_message` event's, is a list of blocks, each with a `type`:

| Block | Fields | What the model sees |
| --- | --- | --- |
| `text` | `text` | The text. |
| `image` | `mime_type`, `data` (base64) | The image. |
| `resource` | `uri`, `text`, optional `mime_type` | A text resource embedded whole, such as an attached file: `text` in a code fence labelled with `uri`. |
| `resource_link` | `uri`, `name`, optional `title`, `description`, `mime_type` | A resource named but not included: `description`, else `name`, in a code fence labelled with `uri`. |

A prompt with no blocks gets `InvalidParams`. A block of a type the daemon does not know reaches the model as a fenced placeholder. A `user_message` holds the message as saved, so a resource the prompt carried appears as its fenced `text` block, live and on replay.

## Following a session

After `rho/session/attach`, the daemon sends the session's events as `rho/session/event` notifications, each with the `session`, a `seq`, and the `event`. The attach response comes first. Each event's `type` is one of:

| Event | What it carries |
| --- | --- |
| `state` | The job's new `state`. An ended turn's `turn_ended` comes first. |
| `user_message` | A user message, whole: its `message` id and `content`. |
| `agent_message_chunk`, `thought_chunk` | More `text` of the agent's reply or reasoning, under a `message` id. |
| `tool_call` | Opens a tool call, or updates the call with the same `id`: its title, kind, status, input (`raw_input`), content, locations, and, once it finishes, `raw_output`, the text the model got from it. Absent fields are unchanged. |
| `plan` | The agent's plan, replacing any earlier one. |
| `usage` | How full the model's context is: `used_tokens`, and `window_tokens` when the model's window is known. Once a request in the session had a price, `cost` holds what the session has cost so far, across daemon restarts: `amount_micros`, in millionths of `currency` (`USD`). |
| `title_changed` | The session's generated `title`. It arrives a few seconds after the first answered turn. |
| `compaction` | A context compaction began or ended, under its `id`. Its `status` has a `type`: `started`, then exactly one of `completed`, `cancelled`, or `failed` with an `error`. |
| `response_discarded` | The provider's response broke off and is retried; drop what the turn streamed since its last step began. |
| `card` | A titled card shown outside the conversation, such as a background result: its body as `markdown`, as `plain` text for hosts that show text as is, and its `tone`: `neutral`, `accent`, `success`, `warning`, or `error`. |
| `notice` | Something to tell the user that is not part of the conversation. |
| `input_requested` | The turn waits for a person: the `request`, with its `id`, what it `ask`s ([below](#answering-a-turn-s-requests)), the `tool_call` that asks, if one does, and its `timeout`, if it has one ([below](#timeouts)). |
| `input_resolved` | The request `id` closed, with its `resolution`: `answered`, `withdrawn`, or `timed_out`. |
| `input_held` | The request `id`'s timeout stopped; it waits for an answer. |
| `turn_ended` | The turn ended, with its `outcome` ([below](#how-a-turn-ends)). |

Each loaded session has one stream of events, numbered by `seq` from 1. A client keeps the `stream` and the highest `seq` it has handled since its attach response as its cursor, and passes them as `after` when it attaches again, for example after the TUI is closed and reopened. A session the daemon [unloaded](/daemon#unloading-idle-sessions) and reopened since has a new stream. The response's `replay` says what follows:

- `tail`: the daemon still buffers every event after the cursor, and sends those, then the live ones. The client continues where it was.
- `reset`: the cursor is from another stream, too old, at 0, or absent. The client drops what it shows and its cursor. The daemon sends the session's saved conversation, then the events since the last turn saved: the last turn's `turn_ended` and `state`, and the turn under way. The saved conversation's events carry `seq` 0, except the last, which carries the `seq` the turn under way began after, so a client that detaches before it has the whole conversation holds no cursor and resets again.

The first `replay_events` events after the response are the replay; the rest are live. The replay ends in the response's `state`, so a client that attached after a turn ended finds how it ended at the replay's end, and one that attached during a turn waits for a live `turn_ended`. `rho attach` follows a session this way.

The daemon buffers up to 16 MiB of a session's recent events for clients that resume. A turn larger than that still resets correctly once it is saved; a client that resets while it is under way gets a notice that its start is missing.

The daemon detaches a client with `rho/session/detached`, after which no event of that session arrives:

- `lagged`: the client fell 4096 events behind. A turn never waits for a client; attach again with the cursor to catch up.
- `daemon_stopping`: the daemon is stopping.

Attaching a session the connection already follows replaces the earlier attachment.

### How a turn ends

A turn that ends sends one `turn_ended` event; the `state` that records the job's end follows, unless the daemon could not record it. A turn the daemon's stop cuts short sends none: the stop first detaches every client with `daemon_stopping`, and `rho/job/list` shows the job `interrupted`, as it does for a turn whose daemon died. The `outcome` has a `type`:

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

## Signing in and out

`rho/auth/key` and `rho/auth/logout` change the credential store the daemon's sessions read, as `/login` and `/logout` do in the TUI. The daemon moves to the store its config selects when the request arrives and writes that one; a request it refuses writes nothing. Moving stores alone changes no running session's credentials. A change applies to every session the daemon runs, whichever client opened it: a running session uses a new key, or drops a deleted one, from its next prompt, and new sessions use it at once. After a logout, a session's next prompt fails with `TurnEnded`, whose outcome is a `failed` turn of kind `authentication`, until a key is saved again.

`rho/auth/key` first asks the provider's models endpoint about the key and refuses one the provider rejects with `KeyRefused`, keeping the key saved before. A key the provider cannot be asked about is saved anyway, as `rho login` saves it, and the result's `unchecked` says why: its `reason` is `no_model_listing` when rho lists no models for the provider, or `unreachable` when its endpoint could not be reached or failed with another status, and its `message` says so in words, such as `HTTP 404`. A client reads a `reason` it does not know as another reason the key went unchecked. Surrounding whitespace is ignored, and `auth` must take an API key; OAuth and other sign-ins stay with `rho login`. `rho/auth/logout` also forgets an Ollama login.

A credential set in the daemon's own environment, such as `OPENAI_API_KEY`, wins over a saved one, so both methods refuse its auth profile with `EnvironmentOverride` before writing anything; its `data.variable` names the variable to unset where the daemon starts. The client's environment plays no part.

The key travels only over the daemon's socket, which only its user can open, and the daemon never logs it, nor the params of any request.

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
| -32001 | `VersionMismatch` | The protocol majors differ; `data` says which side to change. |
| -32003 | `SessionNotFound` | No session is saved under the id. |
| -32004 | `SessionBusy` | The session already has a turn under way, or another job, such as a subagent run, writes the saved session a prompt or an attach would reopen. |
| -32005 | `NotSteerable` | No model request is taking input. |
| -32006 | `SteerRefused` | The running turn refused the steering input. |
| -32007 | `DaemonStopping` | The daemon is stopping and starts nothing new. |
| -32008 | `SessionOpenFailed` | The daemon could not open the session: for example, the config is invalid, the agent a saved session was saved as changed or is gone, or the session is a subagent's or workflow node's, which only its parent continues. |
| -32009 | `TurnEnded` | The turn ended before it saved the prompt; `data` is its outcome. |
| -32011 | `InputNotOpen` | The input request is not open: another client answered it first, it timed out, or the turn withdrew it. |
| -32012 | `SessionInUse` | Another process, such as a TUI or a foreground `rho run`, has the session open, so the daemon cannot reopen it. |
| -32013 | `EnvironmentOverride` | A variable in the daemon's environment holds the auth profile's credential, so saving or deleting one would change nothing; `data.variable` names it. |
| -32014 | `KeyRefused` | The provider rejected the API key, and nothing was saved. |
| -32015 | `CredentialStoreUnavailable` | The credential store could not be opened, read, or written, as when the OS keyring is locked. |

## Versions and compatibility

The protocol has its own version, `major.minor`, independent of rho's; `rho/hello` and `rho/daemon/status` report it. A client and daemon work together when their majors match.

A protocol minor only adds: methods, notifications, error codes, optional fields, and values of open enums. In the schema, one variant of an open enum is marked `x-rho-other` and matches every value the enum does not list, so a client generated from it accepts values a later minor adds; read them as unknown. For an event or content block the catch-all matches any object with a string `type`, so schema validation alone does not reject a malformed known variant: pick the variant by its `type`, then decode it. A client should also ignore fields it does not know, and treat an error code it does not know by its message. Anything else, such as removing, retyping, or constraining a field, making a field required or nullable, or adding a value to a closed enum, comes only with a new major.

The daemon omits an optional field that has no value rather than sending `null`.

CI holds the protocol to this. A test fails when `protocol.json` no longer matches the wire types, and a check compares it with the schema of the last rho release: a change a minor does not allow fails unless the major rises, and any other change to the wire, which excludes descriptions and names, needs the minor to rise.
