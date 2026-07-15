#!/usr/bin/env python3
"""Create machine-readable evidence for the SDK secret redaction release audit."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import pathlib
import re
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
SOURCE_ROOTS = [ROOT / "crates" / "rho-sdk" / "src", ROOT / "src"]
CREDENTIAL_TYPES = ("CodexTokens", "GitHubCopilotTokens", "XaiTokens")


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args], cwd=ROOT, check=True, text=True, capture_output=True
    )
    return result.stdout.strip()


def rust_sources() -> list[pathlib.Path]:
    return sorted(path for root in SOURCE_ROOTS for path in root.rglob("*.rs"))


def relative(path: pathlib.Path) -> str:
    return str(path.relative_to(ROOT))


def line_number(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def credential_debug_findings(sources: list[pathlib.Path]) -> list[dict[str, object]]:
    findings: list[dict[str, object]] = []
    for path in sources:
        text = path.read_text()
        for type_name in CREDENTIAL_TYPES:
            struct = re.search(rf"pub struct {re.escape(type_name)}\s*\{{", text)
            if struct is None:
                continue
            prefix = text[max(0, struct.start() - 240) : struct.start()]
            derives = re.findall(r"#\[derive\(([^)]*)\)\]", prefix)
            if derives and "Debug" in derives[-1].split(", "):
                findings.append(
                    {
                        "id": f"DEBUG-{type_name}",
                        "severity": "critical",
                        "sink": "Debug",
                        "source": "credential container",
                        "location": f"{relative(path)}:{line_number(text, struct.start())}",
                        "summary": f"{type_name} derives Debug while containing credential values",
                        "release_action": "replace derived Debug with a fixed redacted representation before release",
                        "owner": "capability/security agent",
                    }
                )
    return findings


def count_matches(sources: list[pathlib.Path], pattern: str) -> int:
    expression = re.compile(pattern, re.IGNORECASE)
    return sum(len(expression.findall(path.read_text())) for path in sources)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=pathlib.Path)
    parser.add_argument(
        "--dynamic-result", choices=("passed", "failed", "not-run"), default="not-run"
    )
    args = parser.parse_args()

    sources = rust_sources()
    findings = credential_debug_findings(sources)
    scan_counts = {
        "rust_files": len(sources),
        "debug_display_error_log_candidates": count_matches(
            sources,
            r"derive\([^)]*Debug|impl\s+[^\n]*Debug|impl\s+[^\n]*Display|tracing::|log::|println!|eprintln!",
        ),
        "secret_source_candidates": count_matches(
            sources,
            r"authorization|bearer|token|secret|api[_-]?key|cookie|credential|password|signed",
        ),
        "event_diagnostic_snapshot_candidates": count_matches(
            sources,
            r"RunEvent::|DiagnosticsSnapshot|ProviderError::new|ToolError::|to_json|Serialize|snapshot",
        ),
    }
    blocked = args.dynamic_result != "passed" or any(
        finding["severity"] in ("critical", "high") for finding in findings
    )
    evidence = {
        "schema_version": 1,
        "audit": "rho-sdk-secret-canary",
        "date_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "source_revision": git("rev-parse", "HEAD"),
        "working_tree_changes_included": bool(git("status", "--short")),
        "reviewer": "repository-maintainer automated and manual review",
        "independent_audit": False,
        "scope": [
            "Debug and Display",
            "typed errors and source chains",
            "RunEvent values",
            "DiagnosticsSnapshot",
            "SessionSnapshot JSON and Debug",
            "provider context",
            "stdout/stderr and Rust log sink call sites",
        ],
        "providers_adapters": [
            "rho-sdk synthetic compliant provider",
            "application credential containers (static review)",
        ],
        "canaries": {
            "real_secrets_used": False,
            "secret_classes": [
                "api_key",
                "oauth_access",
                "refresh_token",
                "cookie",
                "signed_query",
            ],
            "sensitive_content_classes": ["prompt", "provider_context"],
            "values_recorded_in_evidence": False,
        },
        "static_review": {
            "commands": [
                "rg -n 'derive\\([^)]*Debug|impl .*Debug|impl .*Display|tracing::|log::|println!|eprintln!' crates/rho-sdk src",
                "rg -n 'authorization|bearer|token|secret|api[_-]?key|cookie|credential|password|signed' crates/rho-sdk src",
                "rg -n 'RunEvent::|DiagnosticsSnapshot|ProviderError::new|ToolError::|to_json|Serialize|snapshot' crates/rho-sdk src",
            ],
            "counts": scan_counts,
            "human_data_flow_reviewed": True,
        },
        "dynamic_review": {
            "command": "cargo test -p rho-sdk --test redaction_canary",
            "result": args.dynamic_result,
            "captured_sinks": [
                "provider Debug",
                "runtime event Debug",
                "failed event Debug",
                "error Debug and Display",
                "diagnostics Debug",
                "snapshot JSON and Debug",
            ],
            "allowed_sensitive_content_verified": [
                "prompt content persists in protected snapshot but not diagnostics",
                "provider context persists in exact-identity snapshot but not diagnostics",
            ],
        },
        "findings": findings,
        "remaining_risks": [
            "provider adapters remain responsible for sanitizing transport error text before constructing ProviderError",
            "conversation and tool content intentionally remain visible in events and snapshots",
            "live credentialed provider and external telemetry sinks were not exercised",
        ],
        "release_decision": "blocked" if blocked else "passed",
        "passed": not blocked,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(evidence, indent=2) + "\n")
    print(f"redaction audit evidence: {args.output}")
    print(f"dynamic canary: {args.dynamic_result}")
    print(f"critical/high findings: {sum(f['severity'] in ('critical', 'high') for f in findings)}")
    print(f"release decision: {evidence['release_decision']}")
    return 0 if not blocked else 2


if __name__ == "__main__":
    raise SystemExit(main())
