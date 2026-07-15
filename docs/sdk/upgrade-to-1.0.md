# Upgrade guide for Rho 1.0

This guide covers two audiences:

- Rust integrations that previously imported private `rho-coding-agent` modules
- users upgrading the `rho` CLI and its application-owned config and sessions

::: warning Pre-release guide
Rho 1.0 and `rho-sdk 1.0` have not been published. This guide records the intended and currently implemented migration boundaries. Final release notes must list any additional change found during release-candidate testing.
:::

## Rust integration checklist

### Dependency and ownership

Stop treating `rho-coding-agent` application internals as a library contract. Depend on `rho-sdk` and construct ownership explicitly:

```rust
let rho = rho_sdk::Rho::builder()
    .provider(my_provider)
    .build()?;
let session = rho
    .session(rho_sdk::SessionOptions::default())
    .await?;
let outcome = session.complete("hello").await?;
```

- `Rho` owns runtime configuration and shared shutdown state.
- `Session` owns provider-neutral history and one-active-run enforcement.
- `Run` owns streaming work, commands, events, and cancellation.
- The host owns provider clients, credentials, tools, persistence, display, and external resources.

### Callbacks to events

Replace mutable application callbacks and TUI messages with `Session::start` and `RunEvent`. Drain events, handle unknown future variants with a wildcard, and use `Run::outcome` for final content, usage, stop reason, and revision. Do not reconstruct the result from deltas.

`Session::complete` cannot answer host questionnaires. Use a streaming run when tools may request input or when the host must approve or steer work.

### Providers

Implement the public `ModelProvider` trait and return explicit `Send` futures. Normalize requests, events, usage, and sanitized errors. Inject credentials through your adapter. Do not rely on SDK environment lookup, keychain access, or application login UI.

The core SDK currently provides only a scripted test provider, not automatic construction of the Rho application's production providers.

### Tools and security

Implement `Tool`, validate JSON arguments, and call `ToolContext::authorize` before each file, process, or network operation. The SDK does not register coding tools and defaults to `DenyAllPolicy` plus `DenyApprovals`.

This is an intentional security difference from importing an application bootstrap that registered tools based on CLI policy. Adding a `Workspace` scopes paths but does not grant them. Explicitly configure each capability and approval path.

### History and persistence

Replace direct message-vector mutation with:

- `SessionOptions::history` for initialization
- `Session::history` for inspection
- `Session::snapshot` and `SessionOptions::from_snapshot` for persistence
- named `compact`, `reset`, `replace_provider`, and `set_reasoning_level` operations

Do not point the SDK at Rho's application SQLite database. There is no automatic importer. Convert application sessions through an explicit, fixture-tested application adapter when one is available.

### Working directory and instructions

The SDK does not infer a workspace from process current directory. Construct `Workspace` with an absolute existing path. `AGENTS.md`, skills, coding-prompt presets, and prompt discovery are host/application policy and must remain scoped and diagnosable.

### Shutdown and errors

Call `Rho::shutdown`, then settle owned runs and close adapter resources. Run drop is an aborting fallback, not a guarantee that partial output is committed.

Match non-exhaustive typed SDK errors. Do not parse application or provider strings, and do not expose nested transport payloads.

## CLI user compatibility

The intended 1.0 CLI contract keeps the `rho` binary name and installation flow. Current intentional behavior is:

- `rho run` writes only the final answer to stdout
- diagnostics and errors go to stderr
- provider, model, auth, reasoning, `--no-system-prompt`, and `--no-tools` remain application options
- prompt arguments and stdin composition remain supported
- non-interactive runs do not initialize terminal/TUI state
- current working-directory and `AGENTS.md` behavior remains application policy
- Rho 1.0 does not add a JSON Lines event mode; a future machine protocol must be separately opt-in and versioned

No CLI flag removal is intentionally documented at this stage. Any intentional exit-status, output, default, or flag change discovered before 1.0 must be added here and to coordinated release notes before the release candidate is approved.

## CLI tools versus SDK defaults

Do not confuse application behavior with SDK authority:

- The Rho CLI may register its built-in tools according to explicit application policy.
- `rho-sdk` registers none by default and denies sensitive capability requests.
- `--no-tools` affects the CLI invocation. It is not an SDK feature flag.
- An embedded host must deliberately reproduce any CLI tool set, workspace root, limits, and security decisions it wants.

This separation is intentional so adding the library does not implicitly grant shell, filesystem, or network access.

## Config migration

`~/.rho/config.toml` remains application-owned. Rho accepts the grouped format documented in [configuration](/configuration) and continues to read the previous flat format, rewriting it into groups when it saves.

The SDK does not:

- create, discover, migrate, or write the application config
- persist CLI flag overrides
- read `RHO_*` or provider environment variables
- open the OS credential store
- apply display, keybinding, title, update, or TUI settings

An application adapter must map relevant config values into provider, reasoning, compaction, prompt, workspace, and tool options. See the [configuration serialization matrix](/sdk/compatibility#application-configuration-compatibility). Credentials stay outside both config serialization and snapshots.

Back up config before testing a release candidate because the application can normalize and rewrite older formats. A config rewrite is not an SDK snapshot migration.

## Session migration

Rho application sessions and SDK snapshots are different persistence contracts:

| Existing data | 1.0 treatment |
| --- | --- |
| Application SQLite session | Remains application-owned until an explicit adapter converts it |
| SDK `SessionSnapshot` schema 1 | Restore through `SessionOptions::from_snapshot` |
| Legacy provider-neutral `Message` JSON | Preserved inside supported snapshot history representations |
| Provider-native context | Require each provider adapter to omit it unless provider/API/model identity matches exactly; report handoff omissions |
| Raw reasoning | Do not migrate into persistent SDK history |
| Credentials or approval state | Never migrate into snapshots |

Do not claim historical-session migration support until fixture tests cover the supported source versions. Export or back up important application sessions before release-candidate testing.

## Security changes to review

Embedded hosts must explicitly review these intentional changes:

1. No sensitive capabilities are enabled by default.
2. No ambient config, environment, keychain, current-directory, or session discovery occurs.
3. Workspace scope and permission are separate decisions.
4. Approvals are allow-once or deny and are cancellable; no remembered grant is implied.
5. Snapshots are sensitive content, not redacted logs.
6. `Debug` output may reveal content and must not be used as a production logging policy.
7. Provider-native context is retained for exact replay but is opaque and sensitive.
8. Cancellation is cooperative and cannot roll back external side effects.
9. Dropping a run aborts work and may skip partial-history persistence.
10. Provider and tool adapters are trusted host code and must enforce their own cleanup and redaction.

## Before switching to 1.0

- pin and test a release candidate
- compile all downstream feature combinations
- test provider/tool contracts without network credentials where possible
- run cancellation and shutdown tests around every external resource
- validate snapshots and application-session conversion with backups
- compare CLI stdout, stderr, and exit status in automation
- review the [security model](/sdk/security) and [threat model](/sdk/threat-model)
- complete the [redaction audit](/sdk/redaction-audit)
- report integration feedback through the [release-candidate process](/sdk/release-candidates)
