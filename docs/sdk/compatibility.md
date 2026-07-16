# SDK compatibility and public contracts

## Published and intended versions

`rho-sdk 1.0.0` is published on [crates.io](https://crates.io/crates/rho-sdk). This is the first stable version, and this page documents its actual public contract rather than a forward-looking intent.

## Stability labels

At 1.0:

- the documented public API, snapshot schema rules, event lifecycle, cancellation/drop/shutdown behavior, capability defaults, and other behavior explicitly marked as contract are stable
- breaking changes to that contract require a major version bump, not a minor or patch release
- consumers should pin a `1.x` version range and read the [upgrade guide](/sdk/upgrade-to-1.0) when migrating from a pre-1.0 checkout
- provider-private wire formats and upstream service behavior are never stabilized by the core crate
- some documented behavior did not fully meet the drafted release-candidate gates before publication; see [known limitations](/sdk/events-and-cancellation#known-limitations)

Internal modules, task/channel implementation details, exact allocation, delta chunk boundaries, provider-private payloads, application UI, and undocumented formatting remain implementation details.

A public item being exported does not by itself make every derived representation a persistence contract. The durable boundary is explicitly documented below.

## Features and capability defaults

`rho-sdk` deliberately declares an empty default feature set:

```toml
rho-sdk = { version = "1.0", default-features = false }
```

The crate currently has no optional Cargo features. Default,
`--no-default-features`, and `--all-features` therefore select the same minimal
headless surface, and CI tests all three modes. Built-in provider transports,
SQLite persistence, operating-system keychain access, web access, and coding
tools remain application-owned adapters. If any moves into `rho-sdk`, it must
use a named opt-in feature and must not grant workspace capabilities without
explicit runtime configuration.

Removing or renaming a public feature, or moving an existing API behind a
feature, is a breaking change. Adding an opt-in feature is additive unless it
changes existing behavior.

## Runtime and supported platforms

The SDK requires a host-provided Tokio runtime when it creates sessions or
runs. Public provider and tool futures are `Send`, and the SDK does not
initialize a runtime, terminal, logger, credential store, network client, or
global configuration for the host.

CI compiles and tests the workspace on the current GitHub-hosted stable Rust
toolchain for Linux, macOS, and Windows. The release binary matrix additionally
builds Linux x86_64 GNU, macOS x86_64 and arm64, and Windows x86_64 MSVC
artifacts. Other Rust targets may work, but are not part of the supported CI
platform contract.

## Public API inventory

The intentional SDK surface is grouped below. Application provider transports,
login flows, `Config`, OS credential storage, the TUI, and built-in coding tools
are not SDK exports.

### Crate-root runtime surface

- Runtime construction: `Rho`, `RhoBuilder`, `SystemPrompt`, `ShutdownOutcome`
- Sessions and runs: `SessionOptions`, `Session`, `SessionState`, `UserInput`,
  `Run`, `RunEvent`, `RunOutcome`, `StopReason`
- Cancellation and reasoning: `CancellationToken`, `ReasoningLevel`,
  `ParseReasoningLevelError`
- IDs: `SessionId`, `RunId`, `ToolCallId`, `HostInputId`, `Revision`, `InvalidId`
- Host input: `HostChoice`, `HostQuestion`, `SelectionMode`, `HostInputRequest`,
  `HostInputResponse`
- Diagnostics: `DiagnosticsSnapshot`, `PromptSource`, `PromptSourceKind`
- Errors and secrets: `Error`, `ProviderError`, `ProviderErrorKind`,
  `Retryability`, `SecretString`
- Compaction: `CompactionPolicy`, `CompactionRequest`, `CompactionOutput`,
  `CompactionOutcome`, `CompactionState`, `CompactionTrigger`, `Compactor`,
  `CompactionFuture`, `ScriptedCompactor`
- Persistence: `SessionSnapshot`, `SessionStore`, `SessionStoreFuture`,
  `InMemorySessionStore`, and the supported schema-version constants
- Workspace and approvals: `Workspace`, capability and process types, policies,
  approvals, authorization outcomes, and resolved workspace paths
- Tool results and provider activity kind constants re-exported at the root

### `rho_sdk::model`

The provider-neutral model exports are the tool-call, content, message, model
identity, provider-context, request/response/event, and usage DTOs. Their public
fields and variants support provider implementations and serialized history.
Adding a required field or exhaustive variant is source-breaking. Compatible
serialized additions must be optional or defaulted. Provider-native context is
opaque JSON scoped by exact `ModelIdentity`; it must not contain credentials.

### `rho_sdk::provider` and `rho_sdk::tool`

The provider extension surface includes `ModelProvider`, its explicit future
and event-channel types, and scripted downstream test support. The tool
extension surface includes `Tool`, `ToolRegistry`, `ToolInvocation`,
`ToolContext`, output/error/metadata/progress types, security declarations, and
scripted downstream test support. Both extension traits are object-safe, require
`Send + Sync`, and return `Send` futures.

`ToolInvocation::arguments` and `into_arguments` intentionally support borrowed
and owned argument parsing. `ToolRegistry::len` and `is_empty` intentionally
allow hosts to inspect registry construction without relying on tool specs.
These convenience methods are part of the planned stable public surface.

### Application compatibility shims

Private provider construction shims remain tracked for removal by issue #256.
They delegate through explicit application credential sources and do not form
part of the SDK API. New application code must use `ProviderBuildOptions`, an
explicit `ProviderCredentialSource`, and `build_sdk_provider_with_source`.

## Public data behavior

Public values follow these conventions:

| Family | Clone and equality | Serialization | Redaction and `Debug` |
| --- | --- | --- | --- |
| IDs and revision | IDs clone and compare by full string; revisions copy and compare numerically | IDs are transparent non-empty strings; revision is a transparent integer | IDs are visible in `Display` and `Debug`; do not put secrets in IDs |
| Provider-neutral history and usage | Deep clone and structural equality | Serde representation supports snapshots and historical message compatibility | Derived `Debug` can reveal prompts, images, tool arguments/results, summaries, usage, and opaque context |
| `SessionSnapshot` and `CompactionState` | Deep clone and structural equality | Versioned JSON persistence boundary | Derived `Debug` reveals snapshot content; snapshots are sensitive |
| Events and outcomes | Owned, cloneable, structural equality where implemented | Not a versioned wire format in 1.0 unless a separate event format is explicitly introduced | Event `Debug` can reveal model and tool content |
| Builders, runtime, sessions, runs, tokens, senders, and trait objects | Operational handles have type-specific clone/ownership behavior and are not value records | Not serializable | Custom `Debug` is diagnostic only and is not a general secret scrubber |
| Workspace, policy, approvals, host input, tool metadata/output/errors | Structural equality where implemented; policies/handlers are behavior | Not a durable SDK schema | Paths, commands, URLs, diffs, questions, answers, and error text may be sensitive |

Cloning a value duplicates its sensitive data in memory. Equality means structural/value equality, not semantic equivalence, authorization equivalence, constant-time secret comparison, or provider compatibility. `Debug` output is never a safe logging boundary unless the specific type and all nested values have passed the [redaction audit](/sdk/redaction-audit).

Public extensible event, policy, capability, operation, error-category, state, and stop-reason enums are non-exhaustive where compatible growth is expected. Downstream code must retain wildcard branches and must not depend on the current variant count.

## Serialization contract

### Stable persistence boundary

`SessionSnapshot` JSON is the intended versioned 1.0 persistence boundary. Schema version 2 adds an optional opaque prompt-cache key. Schema version 1 remains supported and migrates in memory; serialization always emits version 2. Deserialization rejects malformed, older-than-supported, and newer schemas. New and imported snapshots sanitize raw aborted-assistant reasoning.

Nested provider-neutral message history preserves Rho's historical externally tagged enum representation, including legacy assistant messages, enriched assistants, and aborted assistants. Opaque provider context is data, not a public provider wire contract, and is replayable only to an exact identity.

The following are not promised as durable formats:

- Rust `Debug` output
- `RunEvent` serialization, because events do not currently implement a separately versioned wire protocol
- diagnostics snapshots
- builder or runtime configuration
- host-input channels, approval requests in flight, cancellation state, and active runs
- provider-private HTTP/WebSocket payloads
- the Rho application's SQLite schema or `~/.rho/config.toml`

A compatible snapshot evolution must either remain readable under the same schema with defaults for additive fields or introduce an explicit new schema and migration. It must not silently reinterpret provider context, restore raw reasoning, add ambient authority, or turn metadata into executable configuration. Removing a readable schema or changing the meaning of an existing field is a breaking persistence change and requires a major SDK release after 1.0.

### Application configuration compatibility

The Rho application's `~/.rho/config.toml` is owned by `rho-coding-agent`, not `rho-sdk`. The application currently reads its grouped format and still accepts the previous flat format, rewriting it into groups on a later save. SDK construction never discovers or writes this file.

Application values can affect an SDK-backed run when the application explicitly maps them:

| Application value | Runtime effect | Stored in `SessionSnapshot` |
| --- | --- | --- |
| provider/model/auth | Provider construction and exact model identity | Provider identity and per-message provenance/context are stored; credentials and auth mode are not |
| reasoning | Request policy for future turns | No |
| compaction thresholds/targets | Application compactor and policy construction | Compaction results/state are stored; policy settings are not |
| system prompt and discovered instructions | Prompt construction | Prompt content can appear as a system history message; discovery settings/sources are not stored |
| tools, output limits, web search, RTK behavior | Application tool registry and adapters | Tool calls/results may appear in history; authority and limits are not stored |
| display, keybindings, title model, update checks | Application/UI behavior | No |
| credential-store entries and environment variables | Provider/tool adapter construction | Never |

Changing application configuration does not mutate an existing snapshot. Restoring a snapshot under different runtime options applies the newly supplied runtime for future work while preserving snapshot history. Hosts must validate and disclose that combination, especially provider handoff and newly granted tools.

## Behavioral contracts

The following pages are normative for the stable 1.0 behavioral contract:

- [event ordering, bounded buffering, retry, cancellation, drop, and shutdown](/sdk/events-and-cancellation)
- [history commits, compaction, snapshot atomicity, failure, and restore](/sdk/sessions-and-persistence)
- [tools, workspace paths, process/network limits, and approval behavior](/sdk/tools)
- [security defaults and host obligations](/sdk/security)

Where code and documentation disagree, treat it as a bug to resolve in the next release, not permission to select the less secure behavior. See [known limitations](/sdk/events-and-cancellation#known-limitations) for gaps that shipped in 1.0.0 despite the drafted release-candidate gates.

## Deprecation policy

After 1.0:

1. Public API or behavior planned for removal is marked deprecated in Rust and documented in release notes with a replacement and rationale.
2. Deprecated stable APIs remain available through the rest of the current major release whenever safety and security permit.
3. Removal or a breaking signature, snapshot, event, capability-default, or documented semantic change requires the next major release.
4. Additive non-exhaustive variants, new optional methods with defaults, and additive opt-in features may ship in minor releases.
5. A security or soundness defect may require faster restriction or removal. The release must clearly identify the exception, impact, migration, and any advisory.
6. Provider or feature deprecations caused by upstream shutdown are announced as early as practical, but an upstream service cannot be kept operational by SemVer.
7. Compatibility shims must have an owner, replacement, and tracked removal target. They are not permanent hidden contracts.


## Minimum supported Rust version

The `rho-sdk` minimum supported Rust version (MSRV) is **1.86**. The
`rho-coding-agent` application MSRV is **1.92** because its terminal,
credential, and terminal-native Mermaid rendering dependencies require a newer
compiler. Both values are declared as
`package.rust-version` in Cargo metadata and tested in CI.

An MSRV increase must not ship as a patch release. It must update Cargo
metadata, this page, and CI together, and release notes must call it out. After
1.0, an SDK MSRV increase requires at least a minor version increase. Emergency
compiler requirements caused by a security or soundness fix may skip normal
notice, but still require coordinated metadata and CI updates.

## Semantic-version and downstream checks

Public Rust items, documented event ordering and cancellation behavior, feature
names, and versioned persisted formats are compatibility contracts. CI runs
`cargo-semver-checks` against the pull-request base when that revision contains
`rho-sdk`. Post-1.0, breaking changes require a major version bump and must not
ship in a minor or patch release.

The excluded workspace in `fixtures/downstream` has its own committed lockfile
and crates with exactly one direct dependency, `rho-sdk`. CI compiles those
representative integrations from their reproducible dependency graph. Because
the fixture is outside the Cargo workspace, the release workflow regenerates
its lockfile after release-please changes the SDK version. It also checks all
SDK feature modes, workspace tests, Clippy, rustdoc tests, architecture rules,
package contents, and publish dry runs. Run the local compatibility checks with:

```sh
python3 scripts/check_sdk_compatibility.py --test-features --test-downstream
```
