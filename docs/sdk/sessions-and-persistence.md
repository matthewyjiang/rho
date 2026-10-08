# SDK sessions, compaction, and persistence

## Session identity and revisions

Every session has a non-empty `SessionId` and a monotonic `Revision`. A successful history commit, cancellation commit, cooperative failure commit, reset, or compaction increments the revision. A `RunOutcome` reports the committed revision so a host can associate output with the exact session state.

One session supports one active mutable operation. `Session` clones share this constraint and state. Use separate sessions for concurrent conversations. Concurrent runs that mutate the same session are intentionally unsupported.

## History commit contract

A run starts from a cloned history and appends user input to a private candidate. The contract is:

| Outcome | History behavior |
| --- | --- |
| Successful final answer | Commit the user message, completed assistant/tool steps, steering, and final assistant atomically under the session lock |
| Cooperative cancellation completion | Commit recoverable work, including the user message and an `AbortedAssistant` when partial provider output exists, then return `Error::Cancelled` |
| Cooperative terminal failure | Commit recoverable work the same way as cancellation, including an `AbortedAssistant` when partial provider output exists, emit `Failed`, and return the typed error. Read `Session::revision` after the run for the post-commit revision. Provider adapters may add a model-visible abort marker when replaying `AbortedAssistant` |
| Run-handle drop or task abort | No commit, terminal-event, or partial-recovery guarantee |
| Tool success or tool-reported failure | Append a tool result to candidate history and continue the model loop |
| Event-consumer interrupt (nonterminal send failed because the consumer was dropped) | Do not commit uncommitted candidate run history |
| Reset | Replace history with the configured custom system prompt, if any, and clear compaction state |
| Step checkpoint (only with a checkpoint store) | Commit the candidate before each provider request and after each model reply, before its tool calls start, then save the snapshot |

An automatic compaction is its own immediate commit. If a later step fails, that already successful compaction remains committed, and cooperative terminal failure still commits later recoverable candidate progress. Hosts must not assume every failed run leaves the starting revision unchanged.

Raw streamed reasoning is not committed. A provider-produced reasoning summary may be retained. On cooperative cancellation or terminal failure, partial text, summary, provider context, partial tool calls, and usage may be retained in `AbortedAssistant`, but its raw `reasoning` field is cleared before snapshot construction and import.

## Compaction

Compaction transport and policy are host supplied:

- `Compactor` accepts owned provider-neutral history and cancellation, then returns complete replacement history.
- Host compactors may attach optional `ModelUsage`, including `cost_usd_micros`, when the summary step itself was charged.
- `CompactionOutput::with_metadata` attaches host-owned snapshot metadata. Its keys are committed atomically with replacement history, replacing matching keys while preserving unrelated metadata. Buffered completion events therefore carry the state of that checkpoint, not a later live value. Do not derive authoritative metadata from model-written summaries or recognizable user text.
- `CompactionPolicy::after_messages` triggers at or above a nonzero message count.
- `CompactionPolicy::at_context_tokens` triggers when the session's calibrated context estimate reaches a nonzero token threshold. Before a successful provider usage report, it falls back to the local estimate of message and tool-schema context.
- A builder with automatic policy but no compactor is invalid.
- Automatic compaction is checked before each provider step, emits started/completed events, commits the replacement immediately, updates compaction counters, and then continues.
- Compaction accounting accumulates completed operations, removed messages, estimated removed context tokens, optional cost, and the latest before/after token estimates.
- `Session::compact` is an exclusive manual operation and returns a typed `CompactionOutcome` with message counts, token estimates, optional cost, and the new revision. It does not create a streaming `Run` event sequence.
- A failed or cancelled compactor does not install its replacement history.

A compactor must preserve valid conversation structure and all information the host requires for continuation. The SDK does not prescribe a summarization model. Repeated compaction must remain bounded and should be tested with the host's actual policy.

### Live request context

`RhoBuilder::request_context(source)` installs a host-owned `RequestContext`. Its synchronous `messages(&SessionId)` method returns current provider-neutral context. Implementations must be cheap and must not read or mutate SDK sessions. Use the session ID to isolate state if a runtime serves multiple sessions.

The SDK appends this context to provider requests without adding it to conversation history or snapshots. The host owns its persistence and must restore the source before continuing a saved session. Rho uses this for the latest task checklist, including updates made inside `codemode`: summaries need not reproduce it, and stale copies do not accumulate in history.

The Codex adapter tracks the durable conversation prefix separately from trailing model context. Incremental requests send new conversation items plus the current context, so checklist updates do not force a full replay. Earlier context can remain in the server-side response chain, but the fresh projection supplies the latest state. Pending steering auto-continuations are reused only when the context is unchanged; a changed projection requires replay because the server has already begun generating with the old context. Rewritten conversation history still invalidates continuation. Context accounting calibration is independent of transport continuation reuse.

Construct data messages with `Message::model_context(text)`. Providers and other request readers can recognize them with `Message::as_model_context()`; this is attribution, not authentication. The text is data, not a new user request or a grant of authority.

Request context participates in token estimates and provider calibration. Compactors receive raw history and must reserve the mandatory context's footprint when sizing replacement history; the context source itself is not summarized.

Sources that need request-aware sizing can implement the optional `RequestContext::prepare` callback. Before automatic compaction evaluation, manual compaction, and provider projection, it receives the actual history (including incoming user input and tool-loop results), advertised schemas, and current `ContextEstimate`. Convert a model-token capacity with `ContextEstimate::estimated_budget` before comparing it to local token estimates. Preparation is synchronous, may repeat for a boundary, and runs outside SDK session locks; do not call back into SDK sessions. Idle accounting reads do not prepare or mutate the source. This shared boundary applies equally to interactive, ACP, and automation hosts.

### Shared context accounting

`Session::context_estimate()` returns the committed estimate while idle and the latest published history-boundary estimate during a run. It is a small snapshot, not a copy of live history. `ContextEstimate::tokens()` uses the last applicable successful request's inclusive prompt usage plus locally estimated appended messages. It never uses accumulated multi-request billing usage. `estimated_tokens()` exposes the uncalibrated estimate; the provider baseline and its corresponding local estimate are available separately.

Calibration applies only when conversation history preserves the measured prefix, request-only context is unchanged, and provider identity and tool schemas still match. Reset and explicit history replacement invalidate it. Compaction also invalidates it when the replacement rewrites the measured prefix; unchanged compactor output preserves the baseline. Completed-operation counters retain their existing meaning even when an operation does not reduce history. Failed or cancelled provider attempts do not establish a new baseline; delivered steering invalidates calibration when the exact request cannot be reconstructed. Baselines are not serialized, so resume starts uncalibrated until a fresh successful report. `Session::estimate_context(messages)` lets hosts size proposed history, such as a pending user prompt, against the same accounting without mutating the session.

`Session::context_breakdown()` itemizes the same request for diagnostics: request framing, each advertised tool schema, each history message (system prompt, user, compaction summary, assistant, or tool result attributed to its tool by call id), and each request-only context message. Parts use the local estimator, so they sum to the returned estimate's `estimated_tokens()`; hosts that show per-source numbers next to the calibrated `tokens()` total must scale them. It scans history under the session lock, so call it on demand rather than per frame. During a run it covers committed history only.

`CompactionRequest::context_estimate()` supplies optional accounting for the history being compacted. Host compactors that partition history with the local estimator can use `ContextEstimate::estimated_budget(target_tokens)` to convert a model-token target into a conservative local budget. This conversion is approximate and never enlarges the target. Manually constructed requests may omit accounting. When fresh boundary input must remain verbatim, the request's estimate describes only the compactable prefix, while the trigger still checks the full context.

Requests from `Session::compact()` and automatic compaction also carry the session's prompt cache key (`CompactionRequest::prompt_cache_key()`) and the tool specs its provider turns advertise (`CompactionRequest::tool_specs()`). A compactor that summarizes with the session model can send the history under those settings so the provider serves the prefix from its prompt cache. Both are `None` on manually constructed requests unless set with `with_prompt_cache_key` and `with_tool_specs`.

`Session::last_compaction_decision()` exposes the last SDK policy check, including disabled policy, below-threshold, and pending-async-tool skips. `CompactionDecision::extent()` reports whether a due compaction covers the whole history or only the prefix before pending async tool calls. A due decision is not a completion event. `StepStarted::estimated_context_tokens` continues to mean the raw local estimate; hosts wanting the calibrated count should query the session. Compaction outcome and snapshot before/after token statistics remain message-only local estimates, not the calibrated trigger or billing totals.

`NEXT_MAJOR(rho-sdk): carry ContextEstimate on StepStarted instead of estimated_context_tokens.` Until that field can change, use the session accessor for the latest accounting snapshot. It can be newer than a buffered event; it is not an event-owned snapshot of that exact step.

## Snapshot schema

`Session::compaction_state()` reads committed compaction counters and the last before/after local-token result without copying history. It includes completed operations that did not reduce context, survives snapshot restore, and is separate from the latest policy decision or any in-flight attempt.

`Session::snapshot` returns `SessionSnapshot`, the stable persistence boundary. Schema version 2 contains:

- schema version
- session ID
- revision
- portable message history
- provider identity
- compaction continuation state
- string metadata
- an optional opaque, non-secret prompt-cache key

Schema version 1 remains readable and migrates in memory to version 2 with no prompt-cache key. Serialization always emits the current schema. JSON import and direct Serde deserialization reject malformed, older-than-supported, and newer schemas rather than guessing a migration.

The schema intentionally does **not** contain:

- provider credentials, authorization headers, or keychain references
- workspace authority, approval decisions, or tool instances
- event buffers, active run state, cancellation handles, or host-input responders
- compactor implementation or automatic compaction policy
- endpoint clients, process handles, terminal state, or logging configuration
- raw reasoning

A snapshot does contain conversation content, system and user prompts, tool calls and results, image data, reasoning summaries, provider identity, metadata, and opaque provider context. Treat the complete snapshot as sensitive.

## Provider context on restore

Provider-native context remains in snapshot history tagged with the exact provider/API/model identity that created it. Restoring with another identity does not reinterpret or delete it. Canonical SDK history still contains the tagged blocks. Before creating an upstream wire request, a provider adapter must omit incompatible blocks with the handoff helpers or equivalent [native replay filtering](/sdk/providers#provider-native-replay-and-handoff) while preserving portable content. Hosts should surface handoff omissions and may choose to remove provider-native blocks as a retention policy.

`SessionOptions::from_snapshot` restores ID, history, revision, and compaction state, and avoids inserting the runtime's system prompt a second time. The runtime's currently configured provider executes future turns; the snapshot's provider identity is compatibility metadata, not an instruction to acquire credentials.

## Store and atomicity responsibilities

The SDK exposes a `SessionStore` interface that loads and atomically replaces complete snapshots. Its `Send` futures allow durable adapters to move blocking work off the runtime thread. A failed save must leave the previous complete snapshot loadable. The included `InMemorySessionStore` implements this contract for examples, tests, and simple hosts by replacing a snapshot while holding one mutex.

A durable host adapter should:

1. serialize a complete snapshot
2. write a new record or temporary file
3. flush as required by its durability promise
4. atomically replace or commit the previous revision
5. retain the prior revision when serialization or storage fails
6. use optimistic revision checks if multiple processes can write
7. encrypt or otherwise protect sensitive content at rest
8. make retention, deletion, backup, and export behavior explicit

By default the SDK does not call a store. The host chooses snapshot timing. A process crash after an in-memory run commit but before host persistence can lose that latest revision.

## Step checkpoints and continuing a run

`Session::set_checkpoint_store` installs a store that a run saves to at each step boundary: before every provider request and after every model reply, before that reply's tool calls start. A failed save fails the run. A snapshot saved after a reply names every tool call that may have started.

After a crash, restore the last snapshot with `SessionOptions::from_snapshot` and call `Session::continue_history`. It adds no user message. Tool calls without a result are settled first. A built-in tool whose declared capabilities are all `Read` runs again. Every other call gets an interrupted tool result, so the model sees that it may or may not have taken effect. The run then asks the model for the next step.

## Migration and compatibility

Historical Rho application JSONL session schemas 1 and 2 are migrated by the application adapter to complete SDK snapshots. Current JSONL schema 3 stores a complete snapshot base followed by atomic records containing only newly appended model history and the display-history update. History replacement, such as compaction, writes a new complete base. The versioned SQLite index remains application-owned. Historical JSONL and SQLite fixtures cover every supported application schema.

Compatibility rules are documented in [public contracts](/sdk/compatibility#serialization-contract). The historical [1.0 upgrade guide](/sdk/upgrade-to-1.0) distinguishes application config, application sessions, and SDK snapshots for the original cutover. Current schema rules live on this page and in [compatibility](/sdk/compatibility).

## Export safety

Before export, decide whether the recipient should receive:

- system and project instructions
- user or assistant content
- file contents, diffs, command output, and URLs in tool records
- base64 image data
- extracted document text, filenames, MIME types, truncation notices, and extraction warnings
- provider-produced summaries and opaque replay blocks
- custom metadata

A JSON snapshot is a data export, not a sanitized transcript. Run the [redaction audit procedure](/sdk/redaction-audit) against any persistence adapter or export path.
