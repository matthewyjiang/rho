# SDK sessions, compaction, and persistence

## Session identity and revisions

Every session has a non-empty `SessionId` and a monotonic `Revision`. A successful history commit, cancellation commit, reset, or compaction increments the revision. A `RunOutcome` reports the committed revision so a host can associate output with the exact session state.

One session supports one active mutable operation. `Session` clones share this constraint and state. Use separate sessions for concurrent conversations. Concurrent runs that mutate the same session are intentionally unsupported for 1.0.

## History commit contract

A run starts from a cloned history and appends user input to a private candidate. The contract is:

| Outcome | History behavior |
| --- | --- |
| Successful final answer | Commit the user message, completed assistant/tool steps, steering, and final assistant atomically under the session lock |
| Cooperative cancellation completion | Commit recoverable work, including the user message and an `AbortedAssistant` when partial provider output exists, then return `Error::Cancelled` |
| Run-handle drop or task abort | No commit, terminal-event, or partial-recovery guarantee |
| Tool success or tool-reported failure | Append a tool result to candidate history and continue the model loop |
| Fatal provider, policy, event-delivery, or run error | Do not commit uncommitted candidate run history |
| Reset | Replace history with the configured custom system prompt, if any, and clear compaction state |

An automatic compaction is its own immediate commit. If a later step fails, that already successful compaction remains committed even though later candidate messages do not. Hosts must not assume every failed run leaves the starting revision unchanged.

Raw streamed reasoning is not committed. A provider-produced reasoning summary may be retained. On cancellation, partial text, summary, provider context, partial tool calls, and usage may be retained in `AbortedAssistant`, but its raw `reasoning` field is cleared before snapshot construction and import.

## Compaction

Compaction transport and policy are host supplied:

- `Compactor` accepts owned provider-neutral history and cancellation, then returns complete replacement history.
- `CompactionPolicy::after_messages` triggers at or above a nonzero message count.
- `CompactionPolicy::at_context_tokens` triggers when the SDK's provider-neutral estimate of message and tool-schema context reaches a nonzero token threshold.
- A builder with automatic policy but no compactor is invalid.
- Automatic compaction is checked before each provider step, emits started/completed events, commits the replacement immediately, updates compaction counters, and then continues.
- `Session::compact` is an exclusive manual operation and returns a typed `CompactionOutcome`. It does not create a streaming `Run` event sequence.
- A failed or cancelled compactor does not install its replacement history.

A compactor must preserve valid conversation structure and all information the host requires for continuation. The SDK does not prescribe a summarization model. Repeated compaction must remain bounded and should be tested with the host's actual policy.

## Snapshot schema

`Session::snapshot` returns `SessionSnapshot`, the stable persistence boundary proposed for 1.0. Schema version 2 contains:

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

Provider-native context remains in snapshot history tagged with the exact provider/API/model identity that created it. Restoring with another identity does not reinterpret or delete it. Canonical SDK history still contains the tagged blocks. Before creating an upstream wire request, a provider adapter must omit incompatible blocks with the handoff helpers or equivalent exact-identity filtering while preserving portable content. Hosts should surface handoff omissions and may choose to remove provider-native blocks as a retention policy.

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

The SDK does not currently call a store automatically after each message. The host chooses snapshot timing. A process crash after an in-memory run commit but before host persistence can lose that latest revision.

## Migration and compatibility

Historical Rho application JSONL session schema 1 is migrated by the application adapter to complete SDK snapshots. Current JSONL schema 2 stores each SDK snapshot and its display-history update in one recoverable record, while the versioned SQLite index remains application-owned. Historical JSONL and SQLite fixtures cover every supported application schema.

Compatibility rules are documented in [public contracts](/sdk/compatibility#serialization-contract). The [1.0 upgrade guide](/sdk/upgrade-to-1.0) distinguishes application config, application sessions, and SDK snapshots.

## Export safety

Before export, decide whether the recipient should receive:

- system and project instructions
- user or assistant content
- file contents, diffs, command output, and URLs in tool records
- base64 image data
- provider-produced summaries and opaque replay blocks
- custom metadata

A JSON snapshot is a data export, not a sanitized transcript. Run the [redaction audit procedure](/sdk/redaction-audit) against any persistence adapter or export path.
