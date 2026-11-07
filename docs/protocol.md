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
| `rho/session/new` | `cwd` | Opens a new session whose workspace is `cwd`, an absolute path to an existing directory, and returns its `session` id. |
| `rho/session/prompt` | `session`, `text` | Starts a turn and returns `{}` once the turn has saved the prompt. A session runs one turn at a time; a prompt while a turn is under way gets `SessionBusy`. |
| `rho/session/cancel` | `session` | Cancels the turn under way, if any. The turn ends `cancelled`, or `interrupted` when the daemon is stopping. |
| `rho/session/steer` | `session`, `text` | Adds `text` to the running turn's next model request. Gets `NotSteerable` when no model request is taking input. |
| `rho/session/attach` | `session`, `after`? | Follows the session's events ([below](#following-a-session)). Returns its `stream`, the `replay` that follows (`tail` or `reset`), and its job `state`. |
| `rho/session/detach` | `session` | Stops following the session; no event of it arrives after the response. |
| `rho/session/answer` | `session`, `request`, `answer` | Answers one of the turn's open input requests ([below](#answering-a-turn-s-requests)) and returns `{}` once the turn has the answer. |
| `rho/job/list` | | `jobs`: every job the daemon knows, including those earlier daemons ran, oldest first. Each has its `id`, `kind` (`session`, `subagent`, or `workflow`), `state`, `workspace`, and timestamps; the `session` it writes, absent for a workflow and for a subagent on another runtime; the `parent` job that started it, for a subagent or workflow; the `workflow_run` it drives, for a workflow; the `error` when it failed; and, usually while it is `queued`, its `queue` place: the `budget` it waits on, that budget's `limit`, and its `position`, 1 for the next to start. |

A prompt whose turn ends before it saves the prompt returns an error instead of a result:

- `TurnCancelled` when a cancel arrived before the turn began. Nothing is saved and the model is not called.
- `DaemonStopping` when the daemon starts stopping while the prompt waits for its turn to save it.
- `TurnFailed` with the reason when the turn failed, such as unreadable provider credentials. A turn that stopped unexpectedly answers `the turn stopped unexpectedly; see the daemon log`.

What a session runs, and what its job states mean, is described in [Sessions and jobs](/daemon#sessions-and-jobs).

## Following a session

After `rho/session/attach`, the daemon sends the session's events as `rho/session/event` notifications, each with the `session`, a `seq`, and the `event`. The attach response comes first. Each event's `type` is one of:

| Event | What it carries |
| --- | --- |
| `state` | The job's new `state`, and the `error` when it is `failed`. |
| `user_message` | A user message, whole: its `message` id and `content`. |
| `agent_message_chunk`, `thought_chunk` | More `text` of the agent's reply or reasoning, under a `message` id. |
| `tool_call` | Opens a tool call, or updates the call with the same `id`: its title, kind, status, input, content, and locations. Absent fields are unchanged. |
| `plan` | The agent's plan, replacing any earlier one. |
| `usage` | How full the model's context is. |
| `card` | A titled card shown outside the conversation, such as a background result. |
| `notice` | Something to tell the user that is not part of the conversation. |
| `input_requested` | The turn waits for a person: the `request`, with its `id` and what it `ask`s ([below](#answering-a-turn-s-requests)). |
| `input_resolved` | The request `id` closed, with its `resolution`: `answered` or `withdrawn`. |

Each loaded session has one stream of events, numbered by `seq` from 1. A client keeps the `stream` and the highest `seq` it has handled since its attach response as its cursor, and passes them as `after` when it attaches again, for example after the TUI is closed and reopened. The response's `replay` says what follows:

- `tail`: the daemon still buffers every event after the cursor, and sends those, then the live ones. The client continues where it was.
- `reset`: the cursor is from another stream, too old, at 0, or absent. The client drops what it shows and its cursor. The daemon sends the session's saved conversation, then the events of the turn under way. The saved conversation's events carry `seq` 0, except the last, which carries the `seq` the turn under way began after, so a client that detaches before it has the whole conversation holds no cursor and resets again.

The daemon buffers up to 16 MiB of a session's recent events for clients that resume. A turn larger than that still resets correctly once it is saved; a client that resets while it is under way gets a notice that its start is missing.

The daemon detaches a client with `rho/session/detached`, after which no event of that session arrives:

- `lagged`: the client fell 4096 events behind. A turn never waits for a client; attach again with the cursor to catch up.
- `daemon_stopping`: the daemon is stopping.

Attaching a session the connection already follows replaces the earlier attachment.

## Answering a turn's requests

A turn that needs a person, for an approval, a plan approval, or questions, opens an input request and sends `input_requested` to every attached client. The job is `waiting_for_input` while any request is open. A request waits for as long as it takes: it is never denied because no client is attached, and a client that attaches later finds it among the events of the turn under way.

Any client may answer with `rho/session/answer`. The first answer wins and every attached client receives `input_resolved` with `answered`; a later answer, or one to a request the turn withdrew, gets `InputNotOpen`. A turn withdraws its open requests, with `withdrawn`, when it is cancelled or ends. Request ids are unique within the session for as long as the daemon has it loaded.

Each request's `ask` has a `kind`, which decides the answers it takes; any other answer gets `InvalidParams` and leaves the request open:

| `kind` | What it carries | Answers (`answer.kind`) |
| --- | --- | --- |
| `approval` | The tool or part of rho that asks (`source`), the `operation` it would perform (read or write a path, discover instructions, run a process, reach the network, or load a skill), the policy's `reason`, and the `tool_call` that asks, if one does. | `allow_once`, `allow_for_session`, or `deny` with an optional `reason` for the tool. |
| `plan_approval` | The `plan`, as markdown, and the permission `modes` it may be carried out in. | `approve_plan` with one of those `mode`s, or `keep_planning`. |
| `question` | A `title` and its `questions`, each with an `id`, `prompt`, `choices` (`value`, `label`), whether it takes `multiple` choices or its own text (`allow_other`), and whether it is `required`. | `answers`, mapping question ids to the chosen choices' values or the user's text; `decline`; or `dismiss`. |

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
| -32003 | `SessionNotLoaded` | This daemon has not loaded the session, which may not exist ([#1661](https://github.com/matthewyjiang/rho/issues/1661)). |
| -32004 | `SessionBusy` | The session already has a turn under way. |
| -32005 | `NotSteerable` | No model request is taking input. |
| -32006 | `SteerRefused` | The running turn refused the steering input. |
| -32007 | `DaemonStopping` | The daemon is stopping and starts nothing new. |
| -32008 | `SessionOpenFailed` | The daemon could not open the session, for example because the config is invalid. |
| -32009 | `TurnFailed` | The turn failed before it saved the prompt; the message says why. |
| -32010 | `TurnCancelled` | The turn was cancelled before it saved the prompt. |
| -32011 | `InputNotOpen` | The input request is not open: another client answered it first, or the turn withdrew it. |

## Versions and compatibility

The protocol has its own version, `major.minor`, independent of rho's; `rho/hello` and `rho/daemon/status` report it. A client and daemon work together when their majors match.

A protocol minor only adds: methods, notifications, error codes, optional fields, and values of open enums. In the schema, one variant of an open enum is marked `x-rho-other` and matches every value the enum does not list, so a client generated from it accepts values a later minor adds; read them as unknown. For an event or content block the catch-all matches any object with a string `type`, so schema validation alone does not reject a malformed known variant: pick the variant by its `type`, then decode it. A client should also ignore fields it does not know, and treat an error code it does not know by its message. Anything else, such as removing, retyping, or constraining a field, making a field required or nullable, or adding a value to a closed enum, comes only with a new major.

The daemon omits an optional field that has no value rather than sending `null`.

CI holds the protocol to this. A test fails when `protocol.json` no longer matches the wire types, and a check compares it with the schema of the last rho release: a change a minor does not allow fails unless the major rises, and any other change to the wire, which excludes descriptions and names, needs the minor to rise.
