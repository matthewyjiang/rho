# SDK threat model

## Status and scope

This is a repository-maintained design threat model for the `rho-sdk` 1.0 boundary. It is not an independent audit, penetration test, fuzzing report, or certification. Update it whenever authority, persistence, provider context, tool execution, or host integration changes.

In scope:

- prompt and instruction sources
- custom and built-in tool adapters
- file, process, and network capabilities
- workspace escape
- credentials and provider transport
- provider-native replay context
- events, diagnostics, errors, logs, and `Debug`
- session snapshots and durable adapters
- host questionnaires and approvals
- cancellation, drop, retry, compaction, and shutdown

Out of scope as security boundaries:

- the shipped Rho CLI, automation mode, and interactive TUI, which currently install allow-all workspace policies to preserve existing application behavior
- model alignment or correctness
- provider service internals
- malicious native code loaded into the host process
- operating-system compromise
- physical access and host-specific account security

## Assets

Protect:

- provider, web, source-control, and other credentials
- source code and files inside and outside the workspace
- process execution authority and inherited environment
- network reachability, including internal services and cloud metadata
- prompt, transcript, image, tool, and provider-context content
- session integrity, revision ordering, and persistence availability
- approval integrity and the user's understanding of an operation
- host availability, memory, CPU, disk, process, token, and billing budgets
- audit and diagnostic integrity

The SDK builder defaults to deny, but the shipped Rho application is an explicitly permissive host: its CLI, automation mode, and TUI allow all capability requests. The capability-policy controls in this threat model protect external embedders only when those hosts install restrictive policies and approval handlers. Rho application users must rely on process isolation, workspace selection, and tool-specific validation rather than assuming the default SDK policy is active.

## Trust boundaries

1. **Host to SDK:** the host supplies provider, tools, policy, workspace, approvals, compactor, persistence, and logs. These adapters are trusted in-process code.
2. **SDK to provider:** prompts, history, images, tool schemas, and replay context cross into an upstream service.
3. **Model to tools:** model-produced names and JSON arguments are untrusted input.
4. **Workspace to filesystem:** path strings cross into mutable filesystem state with symlinks and races.
5. **Tool to process/network:** an approved structured request becomes an external side effect.
6. **SDK to host UI:** events, questions, paths, commands, diffs, URLs, and errors become displayed or logged text.
7. **Memory to persistence/export:** snapshots cross into storage, backups, telemetry, or recipients.
8. **Instruction discovery to prompt:** repository files, skills, and fetched content can influence model decisions but cannot grant authority.
9. **Tool batch scheduler:** model calls share one run coordinator. Prepared capability and resource declarations gate overlap, but they do not synchronize other runs or work that survives an invocation.

## Threats, controls, and residual risk

| Area | Threat | Current or required control | Residual risk and host action |
| --- | --- | --- | --- |
| Prompt instructions | Repository, web, or tool content asks for secrets or broader access | Treat instructions as untrusted; enforce capability policy outside prompts; expose prompt sources | Models can persuade users or generate misleading operations; UI must show actual structured requests |
| Custom tools | Tool ignores policy, leaks data, or performs undeclared side effects | Register only trusted code; provide cancellation and structured authorization | In-process code is not sandboxed; isolate untrusted plugins in a restricted process |
| File reads/writes | Traversal, absolute-path escape, symlink escape, race, or ambiguous normalization | Canonical primary and granted roots; strict parent rejection; operation-specific resolution; scoped authorization; immediate revalidation; canonical execution path | Mutable filesystems retain residual TOCTOU risk; use descriptor-relative safe-open or OS isolation for stronger guarantees |
| Process execution | Shell injection, executable substitution, inherited secrets, forked descendants, or unbounded output | Structured shell/direct invocation; explicit cwd, env mode, executable lookup, arguments, timeout, output budget; approval before spawn; process-group/job cleanup | Shell commands and inherited environments remain powerful; hosts should require approval or provide stricter adapters |
| Network | SSRF, redirects, DNS rebinding, credential forwarding, large responses, or internal endpoint access | Separate network capability; exact normalized HTTP(S) host or explicit tool-managed built-in grant; authorize URL-taking built-ins | Host allowlist alone does not prevent all SSRF; enforce redirects, DNS, proxy, IP range, and I/O limits in HTTP adapters |
| Credentials | Secret appears in endpoint, model input, error, event, diagnostics, `Debug`, snapshot, or trace | Credentials are host-injected and absent from snapshot schema; provider adapters must sanitize | Content types can print nested data; complete the redaction audit and avoid general-purpose logging |
| Provider context | Opaque data leaks or replays to an incompatible model | Tagged exact provider/API/model identity; adapter must omit incompatible blocks while lowering upstream requests | Canonical history still contains all tagged blocks and snapshots may contain sensitive provider content; audit each adapter, encrypt, and retain minimally |
| Sessions | Snapshot tampering, unauthorized disclosure, rollback, concurrent overwrite, or schema confusion | Version, session ID, revision, atomic host writes, reject unsupported schema | No built-in signing/encryption/durable transaction; host supplies access control and conflict policy |
| Host approvals | Prompt spoofing, ambiguous command/path, stale decision, dropped responder, or accidental remembered grant | Structured operation and source; exact-session remembered rules; bounded exactly-once channel; cancellation; typed denials; secret-free bounded audit diagnostics | Approval requests are sensitive in memory and users can still approve harmful operations; UI must render actual fields and apply independent redaction |
| Events/logging | Secret or control sequence reaches logs/UI; slow consumer stalls run | Semantic owned events; bounded backpressure; host rendering/redaction | Events contain raw content and are not an audit log; sanitize before sinks and escape displays |
| Cancellation/drop | Child task or process survives; side effect occurs after user cancels | Shared token; runtime cancellation races; run-drop abort; explicit shutdown | Cancellation is cooperative and not rollback; adapters must reap children and reconcile side effects |
| Retry | Duplicate billable call or side effect | Core auto-retry is limited to malformed provider responses; typed retryability | A host retry is a new operation; require idempotency and check revision/side effects |
| Compaction | Summary drops security context or injects altered instructions | Host-supplied compactor returns one non-empty whole-history replacement; SDK installs it atomically | The core checks non-emptiness, not semantic fidelity or full conversation validity; test long and adversarial histories |
| Availability | Slow streams, event consumers, huge tool output/history, repeated compaction, malformed schemas | Bounded channels, step limit, host limits, validation | No universal resource quota; hosts must cap inputs, output, time, storage, and spend |

## Abuse cases

Release and adapter tests should include at least:

- an `AGENTS.md`, skill, tool output, and web page that each asks for credentials or workspace escape
- relative traversal, absolute outside paths, mixed platform prefixes, symlinked files/directories, missing write targets, and parent replacement races
- commands with metacharacters, deceptive summaries, executable lookup collisions, secret environment variables, infinite output, and grandchildren
- URLs with credentials, mixed case, user-info confusion, alternate ports, redirects, internal IPs, DNS changes, oversized bodies, and signed query strings
- duplicate, malformed, late, cancelled, and dropped approval or questionnaire responses
- cancellation during provider connect/stream, event send, tool progress, approval, subprocess, compaction, snapshot write, and shutdown
- malformed provider responses and host retries after uncertain side effects
- snapshots with unknown versions, historical message variants, opaque context for another identity, raw aborted reasoning, oversized data, and conflicting revisions
- logs/errors/diagnostics/events containing seeded canary secrets in every source field

Documentation does not prove these tests have been run. Release evidence must identify each executed command and result.

## Security invariants for 1.0

A 1.0 release candidate must preserve these invariants:

1. No filesystem, process, or network capability is granted by default.
2. No environment, credential-store, config, session, terminal, update, or logging side effect occurs during default SDK construction.
3. Provider and tool adapters do not mutate session history directly.
4. Provider-native replay requires exact identity compatibility.
5. Raw reasoning is not intentionally persisted.
6. Sensitive operations require explicit host code and policy, with cancellation available.
7. A session has no overlapping mutable runs.
8. Event buffering is bounded and stream drop has deterministic cleanup behavior.
9. Unsupported snapshot schemas fail closed.
10. Public errors and diagnostics do not intentionally expose credentials, while content-bearing values are clearly documented as sensitive.

Any exception blocks the release candidate until fixed or explicitly redesigned and reviewed.

## Review cadence

Review this threat model:

- before every release candidate
- when adding a provider, built-in tool, capability, credential adapter, persistence adapter, event sink, or discovery mechanism
- after a security report or upstream auth/transport change
- when platform path/process behavior changes

The reviewer records changed assets, boundaries, threats, mitigations, tests, residual risks, and owners in the release evidence. Then execute the [redaction audit procedure](/sdk/redaction-audit) and the full [release-candidate process](/sdk/release-candidates).
