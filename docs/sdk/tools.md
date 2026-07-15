# SDK tools, workspaces, and approvals

## Tool contract

`Tool` is an object-safe, `Send + Sync` extension point. Each implementation returns a stable `ToolSpec` with a unique name, description, and JSON input schema, and an explicit `Send` future. Duplicate names are rejected when the runtime is built.

A tool receives:

- a `ToolInvocation` with a typed call ID and JSON arguments
- a `ToolContext` with cancellation, bounded progress, optional workspace, authorization policy, approval handler, and host-input access

A tool returns `ToolOutput` on success or a structured `ToolError` on failure. Tool failures normally become failed tool results sent back to the model, not fatal SDK run errors. Hosts observe `ToolFinished` in either case.

The SDK does not validate invocation arguments against the tool's JSON schema before calling it. The implementation must deserialize and validate untrusted model output, reject unknown or ambiguous inputs as appropriate, and cap resource use.

## Presentation and progress

`ToolMetadata` carries structured operation kind, affected paths, command summary, URLs, and a unified diff. `ToolProgress` adds a message and optional completed/total units. Hosts should render these values according to their own UI and disclosure policy.

Metadata is not an authorization decision and is not guaranteed to be secret-free. Do not infer security state from display strings, and do not log command summaries, diffs, URLs, paths, arguments, or output without applying redaction policy.

Progress uses a bounded channel and applies backpressure. A tool should stop expensive progress production when sending reports that the receiver was dropped. Cancellation takes precedence over cosmetic progress.

## Capability defaults

Registering a tool makes its schema available to the model, but does not grant a sensitive capability through `ToolContext`. The defaults are:

| Capability | Default |
| --- | --- |
| Read a path | Denied |
| Write a path | Denied |
| Execute a process | Denied |
| Network access | Denied |
| Host approval | Denied when no handler is configured |
| Workspace root | Absent |

A `Workspace` scopes paths but does not grant read or write access. A `ScopedWorkspacePolicy` must explicitly allow read, write, process, or specific network hosts. Any allowed class can additionally require an `ApprovalHandler`. Current approvals are `AllowOnce` or deny; no remembered session/rule approval is implied.

Every custom tool must call `ToolContext::authorize` immediately before each sensitive operation. Registering custom code is a trust decision because a tool can ignore the context and use Rust or operating-system APIs directly.

## Workspace path rules

`Workspace::new` requires an absolute path to an existing directory, rejects parent traversal in the supplied root, and canonicalizes it.

`Workspace::resolve`:

- accepts paths relative to the canonical root
- accepts absolute paths only when they begin with that root
- removes current-directory components
- rejects parent, root, and platform-prefix components in the relative portion
- resolves lexically and does not require the target to exist

`Workspace::resolve_existing` additionally canonicalizes an existing target and rejects symlinks whose resolved target is outside the canonical root.

These helpers are necessary checks, not a filesystem sandbox. For new write targets, existing parent symlinks and time-of-check/time-of-use races require care. Open or create the target with platform-appropriate no-follow or directory-handle techniques where practical, and revalidate at the point of use. Do not authorize one path and execute against a separately normalized path.

Access outside the primary root has no built-in broad grant in the current SDK. Implement a deliberate host policy or separate trusted tool rather than weakening workspace checks accidentally.

## Process policy

`CapabilityRequest::ExecuteProcess` carries a program and argument vector. It does not currently model environment inheritance, executable resolution, working directory, stdin, output limits, or shell parsing. A process tool must make each explicit:

- resolve the working directory inside the workspace
- use an explicit executable or documented search policy
- pass arguments as arguments instead of concatenating a shell command unless shell semantics are the intended approved operation
- use an allowlisted environment rather than inheriting secrets by default
- cap stdout, stderr, wall time, child count, and input size
- attach cancellation to the process group or platform job object and reap descendants
- request fresh authorization when a repeated call is materially different

The current SDK provides a policy vocabulary, not a built-in process sandbox.

## Network policy

`ScopedWorkspacePolicy::allow_network_host` compares a parsed URL's lowercase host to an explicit host set. It does not by itself constrain URL scheme, port, path, redirects, DNS rebinding, response size, or destination IP ranges. A network tool or HTTP adapter must enforce those additional rules and reauthorize redirects when required.

Host-provided tools are not transformed into SDK built-ins. Their network clients, proxy behavior, certificate roots, and credential forwarding remain host responsibilities.

## Host input and approvals

A tool may request typed host input. The host receives `RunEvent::HostInputRequested` and answers with `Run::respond(request_id, response)`. The runtime validates the response against the request and accepts a pending request only once. Unknown, duplicate, malformed, late, or dropped-requester responses return `InvalidHostResponse`.

Authorization follows this sequence:

1. the tool creates a structured `CapabilityRequest`
2. `WorkspacePolicy::evaluate` returns allow, deny, or require approval
3. a denial returns `Error::PolicyDenied`
4. an approval request invokes the async host handler
5. cancellation races the authorization future
6. `AllowOnce` authorizes only that request; denial returns `PolicyDenied`

Approval handlers should display structured context, apply independent redaction, time out abandoned requests, and never broaden one decision into an unstated persistent rule.

See [security](/sdk/security) and the [threat model](/sdk/threat-model) before enabling tools.
