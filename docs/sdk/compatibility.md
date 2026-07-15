# SDK compatibility and public contracts

## Stability labels

The repository currently contains `rho-sdk 0.1.0`. Until 1.0:

- public APIs and documented behavior are release candidates, not a stable 1.x promise
- breaking changes may occur in a minor `0.x` release
- consumers should pin an exact version or Git revision and read the [upgrade guide](/sdk/upgrade-to-1.0)
- provider-private wire formats and upstream service behavior are never stabilized by the core crate

At 1.0, the stable contract will include the documented public API, snapshot schema rules, event lifecycle, cancellation/drop/shutdown behavior, capability defaults, and other behavior explicitly marked as contract. Internal modules, task/channel implementation details, exact allocation, delta chunk boundaries, provider-private payloads, application UI, and undocumented formatting remain implementation details.

A public item being exported does not by itself make every derived representation a persistence contract. The durable boundary is explicitly documented below.

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

`SessionSnapshot` JSON is the intended versioned 1.0 persistence boundary. It currently uses schema version 1. `from_json` rejects another schema version. New snapshots sanitize raw aborted-assistant reasoning, and imported schema-1 snapshots are sanitized again.

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

The following pages are normative for the implemented pre-1.0 behavior and are candidates for the 1.0 contract:

- [event ordering, bounded buffering, retry, cancellation, drop, and shutdown](/sdk/events-and-cancellation)
- [history commits, compaction, snapshot atomicity, failure, and restore](/sdk/sessions-and-persistence)
- [tools, workspace paths, process/network limits, and approval behavior](/sdk/tools)
- [security defaults and host obligations](/sdk/security)

Where code and documentation disagree before 1.0, treat it as a bug to resolve before release candidates, not permission to select the less secure behavior.

## Deprecation policy

After 1.0:

1. Public API or behavior planned for removal is marked deprecated in Rust and documented in release notes with a replacement and rationale.
2. Deprecated stable APIs remain available through the rest of the current major release whenever safety and security permit.
3. Removal or a breaking signature, snapshot, event, capability-default, or documented semantic change requires the next major release.
4. Additive non-exhaustive variants, new optional methods with defaults, and additive opt-in features may ship in minor releases.
5. A security or soundness defect may require faster restriction or removal. The release must clearly identify the exception, impact, migration, and any advisory.
6. Provider or feature deprecations caused by upstream shutdown are announced as early as practical, but an upstream service cannot be kept operational by SemVer.
7. Compatibility shims must have an owner, replacement, and tracked removal target. They are not permanent hidden contracts.

Before 1.0, deprecation attributes are encouraged for migration clarity but the `0.x` SemVer rules still apply.

## Rust-version policy

No numeric MSRV is currently declared. Until the first release candidate, the supported compiler is current stable Rust. A numerical MSRV must be selected, declared in Cargo metadata, and tested before 1.0.

For the 1.x line, once declared:

- patch releases must not raise MSRV
- a minor release may raise MSRV only when necessary, with prominent release-note notice and CI validation at the new minimum
- the declared `rust-version`, installation docs, and release notes must agree
- optional features included in the support matrix must also build at the declared MSRV

See [installation support status](/sdk/installation#minimum-supported-rust-version) and the [release-candidate gate](/sdk/release-candidates).
