# SDK events, retries, cancellation, drop, and shutdown

This page defines the implemented behavioral contract that hosts should rely on. The crate remains pre-1.0, so changes before 1.0 must be called out in the [upgrade guide](/sdk/upgrade-to-1.0).

## Event ordering and buffering

Each run owns one bounded multi-producer/single-consumer event channel. Capacity defaults to 64 and can be configured to another nonzero value with `RhoBuilder::event_capacity`.

Events for a run are observed in the order the runtime sends them. A normal stream begins with `Started`, then emits step, provider, tool, host-input, usage, and compaction facts as they occur. Rendering is a host concern; events are not terminal-formatted lines.

When the channel is full, runtime event production waits. This bounded backpressure prevents an unbounded queue but means a host that stops consuming can pause provider/tool orchestration. A host can call `Run::outcome` without manually draining all events because `outcome` drains unread events while waiting for the worker.

Hosts must match `RunEvent` with a wildcard because it is non-exhaustive. Delta chunk boundaries are not stable. Concatenate text only for display; use the terminal `RunOutcome` as authoritative final content.

## Lifecycle sequence

The significant ordering rules are:

1. `Started` is first and includes the starting revision.
2. Each provider loop emits `StepStarted` before that step's provider activity.
3. Provider deltas, tool-call assembly, usage, activity, and context updates retain source arrival order.
4. A complete tool call emits `ToolProposed` before execution.
5. An available tool emits `ToolStarted`, zero or more `ToolUpdated`, then exactly one `ToolFinished`.
6. An unavailable tool emits `ToolFinished` with `Unavailable` and no `ToolStarted`.
7. Automatic compaction emits `CompactionStarted` before calling the compactor and `CompactionCompleted` only after committing replacement history.
8. A run that reaches a normal cooperative terminal path emits one of `Completed`, `Cancelled`, or `Failed`.

A terminal event describes the worker result, but `Run::outcome` remains the authoritative typed result channel. `Completed` contains the same successful outcome. Cancellation returns `Error::Cancelled`. Failure returns the typed `Error`; the event contains sanitized text and retryability for observation.

The current pre-1.0 implementation does not yet guarantee a terminal event for every worker exit. Run drop/abort, task panic, failed terminal delivery, and some cancellation or persistence-error races around nonterminal event emission can close the channel without `Completed`, `Cancelled`, or `Failed`. Hosts must treat end-of-stream as "inspect `Run::outcome`," not infer success. Closing these gaps and testing exactly one attached terminal event is a release-candidate requirement.

## Host input and steering

`Run::steer` sends an additional user input to the active run and waits until the worker accepts it. Accepted steering is incorporated at a model-step boundary. It does not mutate completed history independently.

`HostInputRequested` moves the session into `WaitingForHostInput`. `Run::respond` validates a response and delivers it to a matching pending request exactly once. When no requests remain, the session returns to running. A response can fail because the ID is unknown, the shape is invalid, the requester was dropped, or the run no longer accepts commands.

## Retry contract

The core runtime performs one narrow automatic retry: a malformed normalized assistant response is attempted at most twice in total. Before the second attempt it emits `ProviderActivity` with kind `invalid_response_retry`. A response with zero content blocks and malformed tool calls are invalid. A second invalid response fails permanently.

The SDK does **not** automatically retry every retryable provider, transport, tool, policy, compaction, or persistence error. `Error::is_retryable` and `ProviderError` classification tell the host whether an unchanged retry may succeed; they do not authorize replay or guarantee idempotency. A host retry should start a new run only after checking session revision, provider billing implications, tool side effects, and its own idempotency keys.

Tool-reported failures are returned to the model as tool results, so they can lead to another model step without being an SDK transport retry. The model loop ends with a permanent invalid-response error when it exceeds the configured step count.

## Cancellation contract

`Run::cancel` and `Run::cancellation_handle` request cooperative cancellation. Token clones shared with providers, tools, host input, approvals, and automatic compaction observe the same idempotent state. Cancelling one token clone cancels the run; merely dropping a token clone does not.

The runtime races cancellation against provider work, tool work, authorization, compaction, host-input waits, and event sends. Extension implementations must still stop and clean up any child resources they create when their future is dropped or token is cancelled.

When cancellation reaches the cooperative cancellation completion path:

- the runtime stops new model/tool work
- recoverable candidate history is committed
- partial provider output may become `AbortedAssistant`
- raw reasoning is discarded
- the revision increments
- `Cancelled { revision }` is emitted when delivery succeeds
- `Run::outcome` returns `Error::Cancelled`

Cancellation can race with event delivery or other failing work in the current pre-1.0 implementation. In those cases, `Run::outcome` can still report cancellation or interruption without a cancellation commit or terminal event. A 1.0 release candidate must test and resolve these edge paths according to its final terminal-event contract.

Cancellation is not rollback. A tool or remote provider may have completed an external side effect before observing cancellation. Design tools for idempotency and record enough operation identity for reconciliation.

## Drop contract

Dropping an unfinished `Run` requests cancellation and aborts its worker task. The worker guard unregisters the run and returns the session to idle when task destruction completes. Because abortion can prevent the cooperative cancellation commit, run drop does not promise an `AbortedAssistant`, a revision increment, a terminal event, or persistence of partial output. No consumer remains to observe events after the run handle is dropped.

Dropping `Session` or one `Rho` clone does not shut down work still owned by other clones. Dropping the runtime handle is a safe memory/resource fallback, not coordinated application shutdown. Host-owned tasks launched outside the SDK are the host's responsibility.

## Persistence and event-consumer failures

A failure to send a nonterminal event because the consumer is gone interrupts the run. Uncommitted candidate history is not installed. A compaction that already committed remains installed. Events are observational and are not a durable audit log. If events must survive process failure, the host must persist them with its own sequence, transaction, retention, and redaction policy.

The SDK does not automatically persist after each event or commit. See [persistence atomicity](/sdk/sessions-and-persistence#store-and-atomicity-responsibilities).

## Shutdown contract

`Rho::shutdown` is synchronous and idempotent:

- the first call marks the shared runtime lifecycle shut down
- it requests cancellation on all currently registered runs and compactions
- it reports how many runs were registered at that moment
- later calls return a zero/default outcome
- new sessions and runs are rejected with `RuntimeShutdown`
- clones share the same shutdown state

Shutdown requests cancellation but does not asynchronously join every extension-owned child resource. Continue draining owned runs or wait on their outcomes as appropriate, and separately close provider clients, durable stores, process supervisors, and telemetry exporters owned by the host.

## Session state visibility

`SessionState` exposes `Idle`, `Running`, `WaitingForHostInput`, `Cancelling`, `Completed`, and `Failed`. These values are lifecycle observations, not a lock token. Active-run cleanup eventually returns the session to `Idle`, so terminal intermediate states may be brief. Use run outcomes and revisions for durable decisions rather than polling state for event reconstruction.
