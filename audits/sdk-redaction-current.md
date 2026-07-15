# SDK redaction audit evidence

- Scope: SDK and application Rust sources at the revision recorded in `sdk-redaction-current.json`
- Audit type: repository-maintainer review, not an independent audit
- Dynamic command: `cargo test -p rho-sdk --test redaction_canary`
- Static command: `python3 scripts/audit_sdk_redaction.py --dynamic-result passed --output audits/sdk-redaction-current.json`
- Real credentials used: no
- Dynamic canary result: passed
- Release decision: passed

## Covered sinks

The dynamic harness captured provider `Debug`, ordered run events including
`RunEvent::Failed`, typed error `Debug` and `Display`, diagnostics, snapshot JSON,
snapshot `Debug`, and provider context. Secret canaries were absent. Prompt and
provider-context canaries appeared only in the explicitly allowed protected
snapshot, and neither appeared in diagnostics.

The static review inventoried credential terms, `Debug`/`Display`/error/log
sites, events, diagnostics, serialization, and snapshots across `crates/rho-sdk`
and `src`.

## Findings

No critical or high secret-exposure findings remain. Application credential
containers use fixed redacted `Debug` output, direct regression tests cover
those representations, and the combined-branch static and dynamic audit passes.

## Residual risk

Provider adapters must still sanitize transport errors before constructing
public `ProviderError` values. Conversation, tool, and provider-context content
is intentionally sensitive and remains visible in events or snapshots according
to the documented contract. Live credentialed providers and external telemetry
exporters were not exercised by this deterministic audit.
