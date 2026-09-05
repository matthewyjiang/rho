# Runtime boundary inputs

Background notifications are host input, not human steering. A host can install a
fresh `boundary_input_channel()` on an idle session with
`Session::set_boundary_inputs`. Service its receiver alongside the run event
stream. Each request identifies the session, run, and checkpoint.

The runtime requests input before each provider step, after the preceding
synchronous tool batch has settled, and before committing an end-turn response.
While native asynchronous tool jobs remain pending, the pre-provider checkpoint
is deferred. This keeps internal input out of unresolved tool-call continuations.
The final checkpoint runs after those jobs have settled.
It waits for the host's reply. Returning input at the completion checkpoint makes
the runtime request another provider step instead of committing that response.
The completion checkpoint is skipped on the last allowed step, leaving notices
with the host for the next turn rather than accepting input the model cannot read.
This avoids the race inherent in watching `ToolFinished` and then steering a run
that may already have completed.

Pre-provider input is collected after compaction. Input accepted at completion
reaches the next provider step verbatim. Automatic compaction
still evaluates the full context size, but summarizes only the older history and
appends the fresh input unchanged before checkpointing. Consecutive notifications
therefore do not defer compaction indefinitely.

Streaming continues normally. Text can reach the screen before a notification
arrives. The guarantee concerns committed end-turn completion, not the first
streamed token. Cancellation, provider failure, and the configured maximum-step
limit can still stop a run without another provider response.

## Collection and finalization

The host owns pending queues, ordering, deduplication, and notification policy.
At a checkpoint, take a snapshot of pending work and reserve its delivery. Reply
with the collected `UserInput`, or `None` if the snapshot is empty. Await
`BoundaryInputRequest::respond` without racing that reply future against other
host work. Calling `respond` sends synchronously; release any publication lock
before awaiting its returned future. It returns true once the runtime checkpoints
nonempty input into committed session history. Restore reserved work on false.
Accepted input remains recoverable through `Session::snapshot` after run drop or
event-consumer failure. Hosts still own disk persistence. The runtime acknowledges without waiting
for an event consumer or provider, so a host may await this handshake while
servicing the request.

A nonempty batch copies the current history and advances the session revision
before acknowledgement. This is a checkpoint, not a terminal outcome: the session
remains running and the provider still needs to incorporate the input. Empty
checkpoints do not copy or commit history. Hosts should batch pending inputs rather
than reply once per notification.

An empty `BeforeCompletion` reply is the finalization handoff. The CLI holds one
publication gate across all source snapshots and the synchronous reply send.
Child notices, delegated completion visibility, workflow terminals, and process
terminals use the same gate, so a child's earlier notice cannot fall behind its
completion because the collector sampled the queues separately. The runtime will
not request more input for that run. Keep arrivals after the collection snapshot
in the idle queue and wake a subsequent run. Do not discard late arrivals or send
them to a steering handle for the closing run.

The runtime cancels a pending checkpoint when the run is cancelled or its event
consumer disappears. Dropping the host receiver or an unanswered request fails
the run rather than silently permitting completion. Use a fresh channel for each
run, or remove it with `set_boundary_inputs(None)` before a run that has no host
collector.

## Internal input identity

`RunEvent::BoundaryInputApplied` identifies internal input with the originating
parent session and run. It is separate from human steering acceptance and host
questionnaires. Hosts should retain each background source's identity and order
inside the notification body, and label it as runtime context rather than a new
user request.

`NEXT_MAJOR(rho-sdk): represent internal boundary input with a typed history message instead of User blocks.`
The public `Message` enum is exhaustive, so adding a persisted message kind would
break existing hosts. For now, boundary inputs use framed user-role blocks for
provider and persistence compatibility. Match the distinct runtime event to
identify their source rather than inferring human input from the wire role.
