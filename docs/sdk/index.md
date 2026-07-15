# Rust SDK

`rho-sdk` is Rho's embeddable, headless Rust agent runtime. It provides provider-neutral messages, sessions, completion and streaming runs, custom provider and tool extension points, cancellation, host input, compaction, workspace policy, approvals, diagnostics, and versioned snapshots.

::: warning Pre-1.0 status
The crate in this repository is `0.1.0` and is still being stabilized. The pages in this section distinguish implemented behavior from the intended 1.0 compatibility policy. They do not announce a published release candidate or a completed external integration, security audit, fuzzing campaign, or benchmark run.
:::

## Start here

1. Read [installation and support](/sdk/installation) for dependency, runtime, platform, feature, and Rust-version status.
2. Read [concepts](/sdk/concepts) to understand runtime, session, run, and host ownership.
3. Choose or implement a [provider](/sdk/providers).
4. Register only the [tools and capabilities](/sdk/tools) the host intends to grant.
5. Consume [events and cancellation](/sdk/events-and-cancellation) and persist [session snapshots](/sdk/sessions-and-persistence) as needed.
6. Review the [security model](/sdk/security) and [threat model](/sdk/threat-model) before enabling sensitive operations.

## Current capability map

| Capability | Current SDK contract |
| --- | --- |
| Final answer | `Session::complete` returns a typed `RunOutcome` |
| Streaming | `Session::start` returns a bounded, ordered `RunEvent` stream |
| Providers | Public `ModelProvider` trait and deterministic `ScriptedProvider` |
| Tools | Public `Tool` trait, registry, progress metadata, host input, and scoped authorization |
| Sessions | One mutable run at a time, explicit history inspection, reset, provider replacement, and reasoning changes |
| Cancellation | Shared cooperative token, run cancellation handle, runtime shutdown, and safe run-drop fallback |
| Compaction | Host-supplied `Compactor`, optional automatic policy, explicit manual compaction |
| Persistence | Versioned JSON `SessionSnapshot` and an atomic in-memory adapter, with no SQLite requirement |
| Security | No sensitive capability by default; workspace, policy, approval handler, provider, and tools are host supplied |
| Diagnostics | Secret-free-by-contract configuration snapshot, subject to adapter redaction obligations |

## Examples

The repository contains compiling examples for:

- [simple completion](https://github.com/matthewyjiang/rho/blob/main/crates/rho-sdk/examples/simple_completion.rs)
- [streaming](https://github.com/matthewyjiang/rho/blob/main/crates/rho-sdk/examples/streaming.rs)
- [custom providers](https://github.com/matthewyjiang/rho/blob/main/crates/rho-sdk/examples/custom_provider.rs)
- [custom tools](https://github.com/matthewyjiang/rho/blob/main/crates/rho-sdk/examples/custom_tool.rs)
- [cancellation](https://github.com/matthewyjiang/rho/blob/main/crates/rho-sdk/examples/cancellation.rs)
- [image history](https://github.com/matthewyjiang/rho/blob/main/crates/rho-sdk/examples/image_history.rs)
- [snapshots](https://github.com/matthewyjiang/rho/blob/main/crates/rho-sdk/examples/session_snapshot.rs)
- [questionnaires and approvals](https://github.com/matthewyjiang/rho/blob/main/crates/rho-sdk/examples/questionnaire_approval.rs)

Run one from a repository checkout:

```bash
cargo run -p rho-sdk --example simple_completion
```

## Documentation map

- [Installation and support](/sdk/installation)
- [Concepts and ownership](/sdk/concepts)
- [Providers](/sdk/providers)
- [Tools, workspaces, and approvals](/sdk/tools)
- [Sessions, compaction, and persistence](/sdk/sessions-and-persistence)
- [Events, retries, cancellation, drop, and shutdown](/sdk/events-and-cancellation)
- [Compatibility and public contracts](/sdk/compatibility)
- [Security model](/sdk/security)
- [Threat model](/sdk/threat-model)
- [Redaction audit procedure](/sdk/redaction-audit)
- [Upgrade guide for 1.0](/sdk/upgrade-to-1.0)
- [Release-candidate process](/sdk/release-candidates)
