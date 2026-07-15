# Coordinated 1.0 release notes

`rho-sdk 1.0.0` and `rho-coding-agent 1.0.0` were published on 2026-07-15 from [#262](https://github.com/matthewyjiang/rho/issues/262). `rho-coding-agent` was immediately followed by a `1.0.1` patch ([#272](https://github.com/matthewyjiang/rho/issues/272)) that separated the application release process from the SDK's; it carries no SDK or user-facing behavior change.

::: warning Known gap
Not every entry gate in the [release-candidate process](/sdk/release-candidates) was verified before publication. In particular, the terminal-event delivery guarantee described in [known limitations](/sdk/events-and-cancellation#known-limitations) shipped unresolved. Treat this page as an accurate record of what shipped, not proof every gate passed.
:::

## `rho-sdk 1.0.0`

### Embeddable runtime

- Adds a headless `Rho` runtime with explicit builders, sessions, runs, ordered
  events, typed outcomes, cancellation, steering, and coordinated shutdown.
- Supports custom object-safe providers and tools through explicit `Send`
  futures and bounded event delivery.
- Supports typed questionnaires and host responses, usage accounting, manual
  and automatic compaction boundaries, provider handoff reporting, and
  per-session reasoning control.

### Sessions and persistence

- Adds versioned, serializable `SessionSnapshot` values and a storage trait that
  does not require SQLite.
- Preserves provider-native context only for the exact provider, API, and model
  identity that produced it.
- Keeps display-only transcript state and application storage adapters outside
  provider-neutral model history.

### Security

- Grants no filesystem, process, network, credential-store, or persistence
  authority by default.
- Requires explicit workspaces, capability policies, and approval handlers for
  sensitive operations.
- Keeps built-in provider transports, coding tools, SQLite, keychain, web, and
  terminal dependencies outside the minimal SDK crate.

### Compatibility

- Requires Rust 1.86 and a host-provided Tokio runtime.
- Ships an empty default feature set.
- Treats public Rust contracts, documented event ordering and lifecycle
  behavior, feature names, and snapshot schemas as compatibility boundaries.

## `rho-coding-agent 1.0.0`

### SDK-backed execution

- `rho run` and the interactive TUI execute through the same public `rho-sdk`
  runtime used by third-party applications.
- Removes the duplicate private agent/provider/tool loop.
- Keeps terminal rendering, keybindings, configuration mutation, login and OS
  credential UX, updates, SQLite indexing, and built-in tool presentation in
  the application.

### Automation compatibility

- Preserves prompt argument and stdin composition, provider/model/auth/reasoning
  selection, `--no-system-prompt`, `--no-tools`, working-directory and
  `AGENTS.md` behavior.
- Writes only the final answer to stdout. Diagnostics and failures stay on
  stderr.
- Uses exit status 0 for success, 1 for ordinary failures, 130 for SIGINT, and
  143 for SIGTERM after runtime and managed-process cleanup.
- Does not add a JSON Lines mode in 1.0. Rust `RunEvent` remains the typed
  in-process streaming contract, not a versioned serialization format.

### Interactive compatibility

- Preserves semantic assistant/reasoning streaming, tool progress, diffs,
  command and web summaries, questionnaires, steering, compaction, provider
  switching, resumed sessions, scrolling, pickers, composer behavior, and
  terminal restoration through an application-owned event adapter.
- Keeps presentation-only lines and styles out of SDK provider messages and
  contracts.

### Intentional changes

- Sensitive capabilities are represented and authorized explicitly instead of
  being inferred from global application state.
- Provider-incompatible native context is omitted during model handoff and
  reported rather than replayed unsafely.
- Abandoned runs and coordinated runtime shutdown cancel background work.
- Rust 1.88 is the application MSRV.

## Release order

`rho-sdk` and `rho-coding-agent` were published together from the same commit rather than through the fully sequenced coordinated flow in [release candidates](/sdk/release-candidates):

1. `rho-sdk 1.0.0` and `rho-coding-agent 1.0.0` were published in the same release-please run.
2. `rho-coding-agent 1.0.1` followed same-day to separate the application's release automation from the SDK's.
3. Clean install, executable naming, package contents, and supported-platform artifacts have not been independently re-verified against this specific release beyond CI's existing packaging checks.

See [known limitations](/sdk/events-and-cancellation#known-limitations) for behavior that shipped without the full release-candidate evidence this document originally called for.
