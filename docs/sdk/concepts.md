# SDK concepts and ownership

## Runtime

`Rho` is a cloneable handle to runtime configuration and shared lifecycle state. Build it explicitly with `Rho::builder()` and at least one `ModelProvider`. A runtime owns the provider, registered tools, prompt policy, event capacity, step limit, optional workspace and policies, optional compactor, and shutdown state.

Construction is side-effect-free by default:

- `SystemPrompt::None`
- no tools
- no workspace
- `DenyAllPolicy`
- `DenyApprovals`
- no compactor or automatic compaction
- no environment, keychain, filesystem, network, terminal, logger, or update-check access

A custom system prompt is inserted as the first history message for a new session. Restoring a snapshot does not insert it again.

## Session

A `Session` owns a conversation ID, provider-neutral history, a monotonic revision, compaction continuation state, runtime configuration, and explicit lifecycle state. Clones refer to the same mutable session.

One session permits only one active run or manual compaction. A second attempt returns `Error::SessionBusy`. Different sessions created from the same runtime may run concurrently, subject to the provider and host resources.

Hosts can:

- inspect cloned history and create a snapshot
- inspect state, revision, diagnostics, and reasoning level
- reset an idle session
- replace the provider while idle and inspect provider-context omissions
- change reasoning while idle
- run an explicit compactor

History cannot be mutated in place through the public session API. Initial history is supplied through `SessionOptions` and validated run changes are committed by the runtime.

## Run

`Session::start(UserInput)` creates a `Run`, a unique run ID, an ordered event receiver, a command channel, and a shared cancellation token. The run drives one provider/tool loop and is the host's handle for:

- reading `RunEvent` values
- obtaining the final typed `RunOutcome`
- cancelling
- steering with additional user input
- responding to typed host-input requests

`Session::complete` is a convenience path over the same run loop. It drains events and returns the final outcome. Because it has no host interaction callback, it cancels and returns `InvalidHostResponse` if a tool requests host input. Use `Session::start` for questionnaires or other interactive host work.

## Provider turn and step

A run appends the initial user input to a private candidate history and then performs model steps. Before each step it may compact. A provider receives borrowed provider-neutral messages, tool specifications, cancellation, reasoning level, and optional provider-specific cache metadata. It does not receive the session object and cannot mutate history directly.

A step can end in final assistant content or tool calls. Tool calls are proposed and executed in model order. Successful and failed tool results are both returned to the model for a following step. The default maximum is 32 model steps and can be changed with `RhoBuilder::max_steps`. Reaching that budget commits the accumulated history and completes with `StopReason::MaxSteps`, allowing the host to distinguish a resumable runtime limit from the provider's normal `StopReason::EndTurn` completion.

## Host responsibilities

The SDK supplies mechanics, not ambient authority. The embedding host owns:

- selecting and configuring a real provider
- acquiring, storing, rotating, and redacting credentials
- deciding which tools to register
- implementing tool behavior and calling `ToolContext::authorize` before sensitive actions
- selecting workspace and approval policy
- rendering semantic events
- storing snapshots atomically and applying retention or encryption
- responding to host input exactly once
- draining or deliberately dropping runs
- calling shutdown and waiting for host-owned resources
- setting logging and telemetry policy

A custom provider or tool is trusted host code. The SDK cannot prevent it from opening files, spawning processes, or using the network outside `ToolContext`. The host or operating system must enforce sandboxing when plugin code is not fully trusted.

## Diagnostics and prompt sources

`Rho::diagnostics` and `Session::diagnostics` return owned snapshots of effective provider identity, registered tool names, workspace root, prompt-source metadata, event capacity, step limit, compaction threshold, reasoning level, and enabled SDK features. Diagnostics are intended to describe configuration, not contain credentials or prompt bodies.

The current core SDK supports `SystemPrompt::None` and `SystemPrompt::Custom`. Rho coding-prompt construction, `AGENTS.md` discovery, and skill discovery belong to explicit application or future adapter policy. A host that performs instruction discovery must scope it to the configured workspace and expose included sources in diagnostics rather than hiding global discovery.

Continue with [providers](/sdk/providers), [tools](/sdk/tools), and the detailed [run contracts](/sdk/events-and-cancellation).
