# Coordinated 1.0 release-candidate process

## Current status

No release candidate or final 1.0 publication is announced by this document. It also does not claim an external SDK integration, benchmark, fuzzing campaign, semver check, security audit, or registry verification has occurred.

The current repository metadata is not yet sufficient for coordinated independent publication: `rho-sdk` is `0.1.0`, no numeric MSRV is declared, and release automation must be verified for separate `rho-sdk` and `rho-coding-agent` package versions and tags. Treat those as release blockers, not documentation-only completions.

## Roles

Assign before cutting an RC:

- **release lead:** owns checklist, versions, tags, publication order, and rollback decision
- **SDK API reviewer:** owns export inventory, SemVer report, downstream compile fixtures, and rustdoc
- **application reviewer:** owns CLI/TUI parity, config/session migration, packaging, and install flow
- **security reviewer:** owns threat-model update, capability defaults, redaction evidence, and findings
- **platform reviewers:** own Linux, macOS, and Windows evidence
- **integration contact:** coordinates external adopter feedback without exposing their secrets or private data

One person may fill multiple roles, but each gate needs a named owner and evidence.

## Entry gates

Do not tag `1.0.0-rc.1` until all applicable gates pass.

### Public contract

- inventory every exported SDK item and remove accidental exports
- verify trait object safety, `Send`/`Sync`, serialization, equality, cloning, redaction, and extensibility decisions
- build rustdoc and run documentation tests and examples
- run downstream compile fixtures for default, no-default, all, and every supported feature combination
- verify [event/retry/cancellation/drop/shutdown](/sdk/events-and-cancellation) contracts against tests
- verify [snapshot/compaction/persistence](/sdk/sessions-and-persistence) contracts against current and historical fixtures
- run `cargo semver-checks` or an equivalent comparison against the previous RC or baseline once one exists
- resolve compatibility shims or record an owner and removal release

### Security and reliability

- review and update the [security model](/sdk/security) and [threat model](/sdk/threat-model)
- execute the [redaction audit procedure](/sdk/redaction-audit) and attach evidence
- test default-deny capabilities and structured approvals
- test malicious/ambiguous paths and process/network inputs on supported platforms
- test cancellation at await boundaries and verify no orphan provider/tool/process task
- test slow consumers, bounded output/history, repeated compaction, malformed streams, and dropped responders
- complete any planned fuzz/property tests and clearly report exactly what ran
- resolve all critical/high security findings and assign residual risks

### Compatibility and migration

- verify `rho run` stdout, stderr, exit status, stdin/prompt composition, signals, output piping, no-tools, and no-system-prompt behavior
- complete TUI event/command migration and regression tests required by the 1.0 tracker
- validate terminal restoration and resource shutdown
- validate supported application config migrations and historical session fixtures
- review and update the [1.0 upgrade guide](/sdk/upgrade-to-1.0)
- state every intentional CLI, config, session, persistence, provider, and security change

### Support matrix

- choose and declare a numeric MSRV in Cargo metadata and docs
- test that MSRV for all supported SDK features
- run full Linux tests and the agreed macOS/Windows test matrix, not compile checks alone
- verify packaging and installation on supported targets
- record unsupported targets and upstream-dependent provider behavior

### Performance

- define acceptable regression thresholds before measurement
- benchmark startup, simple completion overhead, event delivery, snapshotting, and compaction against an identified baseline
- record hardware, OS, toolchain, commit, sample method, raw results, and interpretation
- fix or explicitly accept material regressions

A checklist entry is not evidence. Link CI runs, local command logs, reports, or review records to the exact commit.

## Cutting a coordinated RC

1. Freeze the candidate commit and stop unrelated merges.
2. Confirm clean working tree, lockfile, package metadata, licenses, readmes, and generated package contents.
3. Set matching prerelease identifiers, for example `rho-sdk 1.0.0-rc.1` and `rho-coding-agent 1.0.0-rc.1`, while preserving independent package versions.
4. Make the application depend on the exact compatible SDK RC version plus the workspace path.
5. Run the complete gate against the frozen commit.
6. Run package and publish dry-runs for each crate and inspect included files.
7. Prepare separate coordinated release notes using the template below.
8. Publish `rho-sdk` RC first.
9. Wait for registry indexing, then create a clean external fixture that installs the exact registry package and runs completion, streaming, tool, cancellation, and snapshot scenarios.
10. Publish `rho-coding-agent` RC against the registry-available SDK RC.
11. Verify clean installation of the `rho` executable through supported package paths.
12. Tag packages independently, for example `rho-sdk-v1.0.0-rc.1` and `rho-coding-agent-v1.0.0-rc.1`.
13. Publish both release records and link them to each other, the frozen commit, evidence, docs, and feedback channel.

Never publish the application first or rely on an unpublished path dependency. If SDK indexing or clean-fixture verification fails, stop before publishing the application.

## Coordinated release-note template

Prepare one note per package. Do not edit generated changelogs manually.

### `rho-sdk 1.0.0-rc.N`

```markdown
## Highlights

## Public API and behavioral contracts

## Security defaults and threat-model changes

## Snapshot/schema and migration notes

## Providers, tools, and feature flags

## Supported platforms, runtime, and MSRV

## Breaking changes since the prior preview/RC

## Known limitations and upstream-dependent behavior

## Validation performed

## Feedback requested
```

### `rho-coding-agent 1.0.0-rc.N`

```markdown
## Highlights

## SDK integration

## CLI and TUI compatibility

## Intentional CLI/config/session/security changes

## Installation and supported platforms

## Breaking changes since the prior release/RC

## Known limitations and upstream-dependent behavior

## Validation performed

## Feedback requested
```

"Validation performed" must list only work actually executed. Link reports rather than saying "audited," "fuzzed," "benchmarked," or "supported" without evidence.

## External integration feedback

At least one real external application must integrate a registry-published RC before final 1.0. A repository example or in-workspace fixture is necessary validation but does not satisfy that external requirement.

Ask the adopter to exercise:

- dependency installation from the registry
- runtime/session construction without application globals
- completion and streaming
- at least one custom provider or tool relevant to the application
- cancellation and shutdown
- snapshot/restore or an explicit decision not to persist
- default-deny capability behavior
- errors, diagnostics, and redaction under the adopter's logging stack
- their supported deployment platform

Record application category, RC version, platform/toolchain, exercised scenarios, API friction, bugs, security feedback, and resulting changes. Publish only information the adopter authorizes. Do not collect credentials, snapshots, proprietary prompts, or private provider payloads.

Feedback is addressed when each item is fixed, documented as intended behavior, deferred with an owner, or rejected with rationale. Material public-contract changes require another RC and repeated relevant gates.

## Promoting to final 1.0

1. Require a quiet period after the latest material RC change.
2. Confirm at least one external integration and all addressed feedback.
3. Re-run all release gates on the final commit.
4. Compare against the last RC with SemVer tooling and explain every difference.
5. Update final coordinated notes and upgrade guidance.
6. Publish `rho-sdk 1.0.0` first.
7. Wait for indexing and verify a clean registry-only integration.
8. Publish `rho-coding-agent 1.0.0` against the released SDK.
9. Verify supported installation flows.
10. Tag independently as `rho-sdk-v1.0.0` and `rho-coding-agent-v1.0.0`.

After launch, version each crate independently. Release only a package whose artifact changed. Treat breaking SDK API, snapshot schema, event, security-default, and documented behavioral-contract changes as major. The application records its compatible SDK range and exact tested SDK in its lockfile.

## Failure and rollback

Crates.io versions cannot be overwritten. If an RC is defective:

- stop dependent publication if it has not started
- yank only when appropriate and document why
- fix forward with the next RC identifier
- revoke exposed credentials and follow security response if secrets leaked
- repeat every affected gate
- preserve evidence and clearly supersede defective release notes

Do not promote a known-defective RC because the final release is expected to fix it.
