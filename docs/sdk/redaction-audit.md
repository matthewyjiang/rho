# SDK redaction audit procedure

## Purpose and status

Use this procedure to audit logs, errors, `Debug`, diagnostics, snapshots, events, provider adapters, tools, and application bridges for secret exposure before a release candidate.

::: danger Current audit is complete but release-blocked
The current repository audit is recorded in
[`audits/sdk-redaction-current.json`](../../audits/sdk-redaction-current.json)
and summarized alongside it. The synthetic canary test passed for SDK Debug,
errors, events, diagnostics, snapshots, and provider context. Static review
found three credential containers with derived `Debug` implementations, so the
release decision remains blocked until the capability/security owner replaces
those implementations and this audit is rerun. This is a repository-maintainer
audit, not an independent audit.
:::

Run the same release gate with:

```bash
./scripts/run_sdk_redaction_audit.sh
```

The command exits nonzero while critical or high findings remain. Maintainers
recording known findings without treating the release gate as passed may set
`RHO_AUDIT_ALLOW_FINDINGS=1`.

## Redaction boundary

Redaction is required for credentials and transport secrets. Conversation and tool content is sensitive but often intentionally present in events and snapshots, so it must be classified and routed rather than blindly called "redacted."

Use these classes:

| Class | Examples | Allowed destinations |
| --- | --- | --- |
| Secret | API keys, OAuth access/refresh tokens, cookies, authorization headers, signed URL query values, private keys, credential-store values | Provider/adapter memory only; never public output |
| Sensitive content | prompts, files, images, tool arguments/output, diffs, commands, host answers, provider context | Explicit model request, host UI, or protected snapshot according to policy |
| Operational metadata | opaque IDs, provider/model names, event kinds, retryability, revision, counts | Diagnostics/logs if policy permits |
| Public | static labels and documented error categories | Any sink |

A value can move to a less trusted class only through an explicit, tested transformation. Hashes of low-entropy secrets and partial-token prefixes are still secret-derived and should normally be excluded.

## Inventory sources and sinks

Create a review table covering every path below and add adapter-specific paths.

### Sources

- environment variables and command-line values
- OS keychain or credential-store reads
- config and model files
- provider request headers, bodies, endpoints, cookies, query strings, and raw responses
- user/system/instruction prompts and image data
- tool schemas, arguments, progress, metadata, output, and errors
- process environment, command lines, stdout, and stderr
- network URLs, redirects, bodies, and errors
- host questions, answers, approval requests, and decisions
- provider-native context and reasoning
- session metadata and snapshots

### Sinks

- `Debug` and `Display`
- `Error::source` chains and panic messages
- `RunEvent`, provider events, diagnostics, and handoff reports
- stdout/stderr and TUI transcript
- structured logs, tracing fields, metrics labels, crash reports, and telemetry
- snapshot JSON, SQLite or other stores, backups, and exports
- test snapshots and fixtures
- HTTP client logging and proxy logs
- release artifacts and support bundles

## Static review

1. Inventory every public type and adapter that can hold a source value.
2. Search for derived or manual `Debug`, `Display`, `Error`, `Serialize`, tracing/log calls, event construction, and diagnostic construction.
3. Trace each secret from acquisition to provider use and confirm it cannot flow to any sink.
4. Inspect URL and header formatting, especially user info, query strings, redirect errors, and nested transport sources.
5. Inspect errors for raw response bodies, request dumps, environment values, command lines, and provider-specific structs.
6. Confirm diagnostics expose identities and settings but not credential values or prompt bodies.
7. Confirm snapshots omit credentials and raw reasoning, while documenting all sensitive content intentionally retained.
8. Confirm opaque provider context is not rendered or logged by default.
9. Confirm tool and approval metadata is treated as sensitive content, not trusted operational metadata.
10. Confirm custom secret wrappers redact both `Debug` and accidental serialization.

Useful repository searches include:

```bash
rg -n 'derive\([^)]*Debug|impl .*Debug|impl .*Display|tracing::|log::|println!|eprintln!' crates/rho-sdk src
rg -n 'authorization|bearer|token|secret|api[_-]?key|cookie|credential|password|signed' crates/rho-sdk src
rg -n 'RunEvent::|DiagnosticsSnapshot|ProviderError::new|ToolError::|to_json|Serialize|snapshot' crates/rho-sdk src
```

Search results require human data-flow review. Their presence is not automatically a finding, and clean grep output is not proof of safety.

## Dynamic canary test

Use distinct synthetic canaries so a leak identifies its source. Never use real credentials.

Example set:

```text
RHO_AUDIT_API_KEY_7f3a
RHO_AUDIT_OAUTH_ACCESS_1c9d
RHO_AUDIT_REFRESH_442e
RHO_AUDIT_COOKIE_88b1
RHO_AUDIT_SIGNED_QUERY_629a
RHO_AUDIT_PROMPT_CONTENT_d120
RHO_AUDIT_TOOL_CONTENT_43aa
RHO_AUDIT_PROVIDER_CONTEXT_901b
```

For every provider and credential adapter in the release matrix:

1. inject canaries into credentials, endpoint/query values, headers, provider error payloads, prompts, tool fields, and provider context
2. exercise success, authentication failure, transport failure, malformed response, streaming, cancellation, tool failure, snapshot, restore, diagnostics, and shutdown
3. capture all test-controlled sinks, including formatted `Debug`/`Display`, error chains, events, stdout/stderr, logs, telemetry test exporters, snapshots, and support bundles
4. assert that secret canaries occur nowhere
5. assert that sensitive-content canaries occur only in explicitly allowed sinks
6. inspect encoded forms when relevant, including URL encoding, base64, JSON escaping, case changes, and common header formatting

Do not dump captured canaries into public CI logs. Tests should report sink name and match class, not the secret value.

## Required focused assertions

At minimum, add or execute assertions that:

- credential-bearing provider and adapter `Debug` uses a fixed redacted marker or omits the field
- provider errors strip headers, query secrets, cookies, raw bodies, and nested transport text
- runtime/session/run diagnostics contain no credential or prompt body
- `RunEvent::Failed` receives only a sanitized error message
- snapshot JSON contains expected conversation canaries but no credential or raw-reasoning canaries
- aborted-assistant reasoning is cleared both on snapshot creation and import
- provider-context canaries are persisted only where documented, omitted on incompatible replay, and absent from diagnostics
- tool errors/progress/metadata and approval requests do not accidentally receive credential canaries
- default CLI final-answer stdout is not contaminated by diagnostics, reasoning, or tools
- cancellation and shutdown errors do not include captured request details

## Finding severity and release gate

| Severity | Example | Release action |
| --- | --- | --- |
| Critical | Credential in event, snapshot, log, diagnostic, error, or published artifact; incompatible provider context sent upstream | Block release; revoke affected real credentials if exposure occurred |
| High | Sensitive content sent to an undocumented remote/log sink | Block release |
| Medium | Sensitive content in an unexpected local debug/support sink; approval omits security-relevant context | Fix or document and obtain explicit security sign-off before RC |
| Low | Overly detailed non-secret metadata or unclear classification | Track with owner and deadline |

Any unresolved critical or high finding blocks the release candidate. A redaction test that was skipped for a supported adapter is missing evidence, not a pass.

## Evidence record

Store a dated audit record outside generated changelogs with:

```text
release candidate:
commit:
date:
reviewers:
providers/adapters/features/platforms covered:
static review commands and result links:
dynamic canary tests and result links:
allowed sensitive-content sinks verified:
findings and fixes:
remaining risks and owners:
final decision:
```

The record must distinguish repository-maintainer review from an independent external audit. Use the phrase "independent audit" only when an independent party actually performed one and a report or appropriate evidence exists.

After evidence is complete, link it from the [release-candidate record](/sdk/release-candidates) and re-run affected checks after every fix.
