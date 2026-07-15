# Coordinated 1.0 release notes

Status: draft release notes for the coordinated `rho-sdk` and
`rho-coding-agent` 1.0 release. Publishing remains blocked on the release gates
listed in the [release-candidate process](/sdk/release-candidates).

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
- Does not add a JSON Lines mode in 1.0. Rust `RunEvent` remains the versioned
  machine-readable streaming contract.

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

1. Publish and verify `rho-sdk 1.0.0`.
2. Update `rho-coding-agent` to the verified registry dependency and lockfile.
3. Publish `rho-coding-agent 1.0.0`.
4. Verify clean installs, executable naming, package contents, documentation,
   checksums, and supported-platform artifacts.
5. Announce both packages together with links to the security model, migration
   guide, and known upstream-dependent provider behavior.

No item in this draft is evidence of publication. Registry verification and
release artifacts must be attached to the final release record.
