# Rust SDK

`rho-sdk` is Rho's embeddable, headless Rust agent runtime. It provides provider-neutral messages, sessions, completion and streaming runs, custom provider and tool extension points, cancellation, retractable steering, host input, compaction, workspace policy, approvals, lifecycle hooks, provider-free tool hosts, usage recording, diagnostics, and versioned snapshots.

The crate is published on [crates.io](https://crates.io/crates/rho-sdk). These pages document the current public API and compatibility policy. For release-to-release changes, use the [SDK changelog](/sdk/changelog).

## Start here

1. Read [installation and support](/sdk/installation) for dependency, runtime, platform, feature, and Rust-version status.
2. Read [concepts](/sdk/concepts) to understand runtime, session, run, tool-host, and host ownership.
3. Choose or implement a [provider](/sdk/providers).
4. Register only the [tools and capabilities](/sdk/tools) the host intends to grant.
5. Optionally wire [hooks](/sdk/hooks) for observation and pre-tool denial.
6. Consume [events and cancellation](/sdk/events-and-cancellation) and persist [session snapshots](/sdk/sessions-and-persistence) as needed.
7. Review the [security model](/sdk/security) and [threat model](/sdk/threat-model) before enabling sensitive operations.

## Current capability map

| Capability | Current SDK contract |
| --- | --- |
| Final answer | `Session::complete` returns a typed `RunOutcome` |
| Streaming | `Session::start` returns a bounded, ordered `RunEvent` stream; always take `Run::outcome` as authoritative |
| Providers | Public `ModelProvider` trait and deterministic `ScriptedProvider` |
| Tools | Public `Tool` trait, registry, prepare/resource-aware parallel execution, progress, host input, scoped authorization |
| Tool host | Provider-free `ToolHost` for the same registry, approvals, and hooks without a model loop |
| Sessions | One mutable run at a time, explicit history inspection, reset, provider replacement, reasoning changes |
| Steering | `Run::steer` and retractable `Run::steer_retractable` / `Run::retract_steering` |
| Cancellation | Shared cooperative token, run cancellation handle, runtime shutdown, and safe run-drop fallback |
| Compaction | Host-supplied `Compactor`, optional automatic policy, explicit manual compaction |
| Hooks | `hooks` module: observer, pre-tool gate, bounded envelopes, host labels |
| Usage | Optional `ProviderRequestUsageRecorder` for physical request accounting |
| Persistence | Versioned JSON `SessionSnapshot` and `InMemorySessionStore`, with no SQLite requirement |
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

The crate [README](https://github.com/matthewyjiang/rho/blob/main/crates/rho-sdk/README.md) mirrors these examples and covers `ToolHost`, retractable steering, and hook labels.

## Documentation map

### Guide

- [Installation and support](/sdk/installation)
- [Concepts and ownership](/sdk/concepts)
- [Providers](/sdk/providers)
- [Tools, workspaces, and approvals](/sdk/tools)
- [Hooks](/sdk/hooks)
- [Sessions, compaction, and persistence](/sdk/sessions-and-persistence)
- [Events, retries, cancellation, drop, and shutdown](/sdk/events-and-cancellation)

### Security

- [Security model](/sdk/security)
- [Threat model](/sdk/threat-model)
- [Redaction audit procedure](/sdk/redaction-audit)

### Reference

- [Compatibility and public contracts](/sdk/compatibility)
- [Performance acceptance](/sdk/performance)
- [SDK changelog](/sdk/changelog)

### Historical material

These pages record the original stable cutover. Prefer the guide and changelog for current behavior.

- [Upgrade guide for 1.0](/sdk/upgrade-to-1.0)
- [1.0 release notes](/sdk/release-notes-1.0)
- [Release-candidate process](/sdk/release-candidates)
