# SDK tools, workspaces, and approvals

## Tool contract and trust origin

`Tool` is an object-safe, `Send + Sync` extension point. Each implementation returns a stable `ToolSpec` and an explicit `Send` future. Duplicate names are rejected when the runtime is built.

A tool receives a `ToolInvocation` and a `ToolContext` containing cancellation, bounded progress, optional workspace, policy, approval, and host-input access. It returns `ToolOutput` or `ToolError`. Failures normally become failed tool results sent back to the model, while hosts observe a typed `ToolFinished` result.

`Tool::security` distinguishes two trust models:

- `ToolOrigin::HostProvided` is the default. It is trusted in-process host code, and SDK policy cannot sandbox it if it ignores `ToolContext`.
- `ToolOrigin::BuiltIn` declares the read, write, process, network, skill, or instruction-discovery classes the adapter enforces.

`DiagnosticsSnapshot::tools` exposes the origin and declared classes. In particular, a host can distinguish a network-capable built-in from a host-provided tool. A declaration is inspectable policy metadata, not an operating-system sandbox.

The SDK does not validate JSON against a tool schema before invocation. Implementations must deserialize hostile model output, reject unknown or ambiguous inputs, and cap resource use.

## Presentation and progress

`ToolMetadata` carries operation kind, paths, command summary, URLs, and unified diffs. `ToolProgress` adds a message and optional units. These are presentation values, not authorization decisions or safe audit values. Do not infer authority from display strings or log tool arguments and output without host redaction.

Progress uses a bounded channel and applies backpressure. Tools should stop cosmetic progress when the receiver is dropped and always give cancellation priority.

## Independent capability defaults

Registering a tool exposes its schema but grants no sensitive authority. The independently evaluated classes are:

| Capability | Default |
| --- | --- |
| Read a path | Denied |
| Write a path | Denied |
| Execute a process | Denied |
| Network access | Denied |
| Load a skill | Denied |
| Discover workspace instructions | Denied |
| Host approval | Denied when no handler is configured |
| Workspace root | Absent |

`ScopedWorkspacePolicy` opts into each class separately. Network URL hosts and tool-managed network built-ins are separate grants. Paths under deliberately attached roots additionally require `allow_outside_workspace_paths`.

Each `CapabilityRequest` contains a `CapabilityOperation` and `CapabilitySource`. The source distinguishes a host-provided tool, built-in tool, and prompt construction. Policy and approval code receives owned structured facts and must never parse a display command, shell preview, or model explanation.

## Workspace path rules

`Workspace::new` requires an absolute native path to an existing directory, rejects parent traversal, verifies it is a directory, and stores its canonical form. Native NUL units are rejected. Windows prefixes are interpreted only on Windows; Unix backslashes and colon characters remain ordinary filename characters.

Relative paths resolve under the primary root. Absolute paths are accepted only when they are component-wise under the primary root or a root deliberately attached with `Workspace::with_granted_root`. A granted root is labeled `PathScope::GrantedRoot`, so policy cannot silently treat it as primary-workspace access.

Use the operation-specific APIs:

- `resolve_for_read` requires an existing target, follows symlinks, returns the canonical target, and rejects any target outside configured roots.
- `resolve_for_write` canonicalizes an existing target or the nearest existing parent of a missing target. Missing reads fail, while missing writes have the explicit `MissingWriteTarget` state.
- `revalidate` checks that the canonical target, parent chain, scope, and missing/existing state did not change while authorization was pending.

Coding-tool adapters authorize the returned `ResolvedWorkspacePath`, revalidate that same object immediately before I/O, and execute against its canonical path. Edit operations additionally open file handles and detect content changes before writes. This reduces check/use disagreement and common symlink swaps, but portable path checks cannot remove every filesystem race. Hosts needing stronger guarantees should use descriptor-relative safe-open APIs or an operating-system sandbox.

Parent traversal is rejected even if lexical normalization would return inside the root. Symlinks into an attached outside root still require both the root attachment and the policy's outside-workspace grant. There is no implicit home-directory, sibling-directory, or absolute-path grant.

## Explicit process context

`CapabilityOperation::ExecuteProcess` carries a `ProcessExecution` with:

- canonical working directory
- `ProcessInvocation`, distinguishing direct execution from intentional shell execution
- executable selection as an exact path or `PATH` search
- an argument vector separate from shell command text
- environment policy as empty, inherited, or an explicit inherited-name list
- output byte limit and optional wall-time limit

An approval UI can therefore identify the shell boundary, executable lookup, cwd, environment inheritance, timeout, and output budget without parsing shell text. Shell text remains available through a dedicated accessor for display or policy, but its `Debug` representation is redacted. Arguments are also omitted from `Debug`.

Rho's built-in shell and background-process adapters authorize these facts before spawning. They use a canonical workspace cwd, closed stdin, explicit shell arguments, bounded output, `kill_on_drop`, Unix process groups or Windows job objects, and descendant cleanup on timeout, stop, drop, or shutdown. Environment inheritance is currently explicit as `ProcessEnvironment::InheritAll`; security-sensitive hosts should require approval or provide a stricter process adapter.

## Network and skill policy

`ScopedWorkspacePolicy::allow_network_host` accepts only parsed HTTP or HTTPS URLs without URL user information and compares a normalized exact host. It does not match suffixes. `allow_network_tool` is a separate grant for a built-in whose destination is internally managed, such as a configured search backend. Redirect, DNS, proxy, destination-IP, credential-forwarding, and response-size controls remain the network adapter's responsibility.

SDK-facing file skills are loaded only from `.agents/skills/<validated-name>/SKILL.md` beneath the canonical workspace, except for embedded built-ins. The path is canonicalized, authorized as `Skill`, revalidated, and then read. Skill authority does not imply ordinary read authority or instruction-discovery authority.

Instruction adapters use `CapabilityRequest::instruction_discovery` with a resolved path and scope. A custom `SystemPrompt` is already constructed host data, so the SDK does not retrospectively inspect or authorize files the host used to build it. Hosts implementing discovery must authorize before reading and list included `PromptSource` values in diagnostics.

## Approvals, remembered rules, and audit diagnostics

Authorization follows this sequence:

1. the tool submits a structured request
2. policy allows, denies, or requires approval
3. remembered approval is considered only after the current policy still requires approval
4. the async host receives `ApprovalRequest`
5. cancellation drops the pending future and returns a typed cancelled authorization error
6. `AllowOnce`, `AllowForSession`, or denial completes exactly once

`AllowForSession` stores only an exact structured-request rule in that session. Changing a path, scope, command, executable, argument, cwd, environment mode, limit, URL, skill, source, or capability requires another approval. Rules are not persisted, copied to another session, or allowed to override a later policy denial.

`approval_channel` provides a bounded host queue. `PendingApproval::respond` accepts one response; repeating it returns the unused decision. Dropping the receiver or responder produces a host denial instead of hanging. Cancelling a run drops the approval wait.

`ToolContext::authorize` returns `AuthorizationOutcome` or `AuthorizationError`, including typed policy, host, and cancellation denial sources. Built-ins convert denials to `ToolErrorKind::PolicyDenied` with a useful capability-specific message. The model receives that failed tool result and can continue, while the host receives typed `ToolCompletion::Failure`.

`DiagnosticsSnapshot::approval_audit` records bounded, ordered, secret-free decision facts: sequence, capability class, and sanitized result. It intentionally excludes reasons, paths, commands, arguments, environment values, URLs, skill names, and request bodies. Full approval requests remain available only to the approval handler and exact remembered rules remain in session memory.

See [security](/sdk/security) and the [threat model](/sdk/threat-model) before enabling tools.
