//! Natural-looking docs proof-plate fixture for matrix mode.
//!
//! Exact-match only. Not used by ordinary PTY scenarios.

use std::fs;

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse},
    provider::ProviderEventSender,
    ProviderError, ProviderErrorKind, Retryability,
};

use super::{completed, completed_tool_call, tool_result};

/// Exact user prompt the docs generator submits.
pub(super) const PROMPT: &str = "Add request IDs to API logs and update the tests.";

const READ_CALL_ID: &str = "docs-demo-read";
const EDIT_CALL_ID: &str = "docs-demo-edit";
const BASH_CALL_ID: &str = "docs-demo-bash";

const TARGET_PATH: &str = "crates/api/src/middleware/request_id.rs";
const TEST_SCRIPT_PATH: &str = "scripts/check-request-id.sh";
const ORIGINAL: &str = "\
pub async fn request_id(mut request: Request, next: Next) -> Response {
    let request_id = resolve_request_id(&request);
    next.run(request).await
}
";
// FNV-1a tag for ORIGINAL via rho_tools::hashline::compute_file_hash.
const ORIGINAL_TAG: &str = "C7A2";
const EDITED_LINE: &str = "    span.record(\"request_id\", request_id.as_str());";
const TEST_SCRIPT: &str = "\
#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' \\
  'running 28 tests' \\
  'test request_id_generated ... ok' \\
  'test request_id_forwarded ... ok' \\
  'test result: ok. 28 passed; 0 failed; 0 ignored'
";

/// Session-title one-shot payload includes the first user turn under this prefix.
const TITLE_PREFIX: &str = "First turn:\n\nUser:\n";

/// Stable session title for the docs proof plate.
const SESSION_TITLE: &str = "Request ID middleware";

const FINAL_RESPONSE: &str = "\
Done. Request IDs now flow through log spans and response headers.
Focused tests cover both generated and forwarded IDs.";

/// Handle docs-demo prompts (and their session titles) when present.
pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    if prompt.starts_with(TITLE_PREFIX) && prompt.contains(PROMPT) {
        return Some(completed(SESSION_TITLE));
    }
    if prompt == PROMPT {
        return Some(stream(request, events).await);
    }
    None
}

async fn stream(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    seed_workspace()?;

    if tool_result(request, READ_CALL_ID).is_none() {
        events
            .send(ModelEvent::OutputDelta(
                "I'll inspect the request middleware and its tests before changing the request path.\n"
                    .into(),
            ))
            .await?;
        return completed_tool_call(
            READ_CALL_ID,
            "read_file",
            serde_json::json!({ "path": TARGET_PATH }),
        );
    }

    if tool_result(request, EDIT_CALL_ID).is_none() {
        // Insert the span.record line after resolve_request_id.
        let input = format!("[{TARGET_PATH}#{ORIGINAL_TAG}]\nPUT >2:\n+{EDITED_LINE}\n");
        return completed_tool_call(EDIT_CALL_ID, "edit", serde_json::json!({ "input": input }));
    }

    if tool_result(request, BASH_CALL_ID).is_none() {
        return completed_tool_call(
            BASH_CALL_ID,
            "bash",
            serde_json::json!({ "command": "bash scripts/check-request-id.sh" }),
        );
    }

    completed(FINAL_RESPONSE)
}

fn seed_workspace() -> Result<(), ProviderError> {
    let cwd = std::env::current_dir().map_err(|error| {
        ProviderError::new(
            ProviderErrorKind::Other,
            format!("docs demo setup: current_dir failed: {error}"),
            Retryability::Permanent,
        )
    })?;
    write_seed_file(&cwd, TARGET_PATH, ORIGINAL)?;
    write_seed_file(&cwd, TEST_SCRIPT_PATH, TEST_SCRIPT)?;
    Ok(())
}

fn write_seed_file(
    cwd: &std::path::Path,
    relative: &str,
    contents: &str,
) -> Result<(), ProviderError> {
    let full = cwd.join(relative);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ProviderError::new(
                ProviderErrorKind::Other,
                format!(
                    "docs demo setup: could not create '{}': {error}",
                    parent.display()
                ),
                Retryability::Permanent,
            )
        })?;
    }
    // Always rewrite so a prior partial run cannot leave a drifted tag or script.
    fs::write(&full, contents).map_err(|error| {
        ProviderError::new(
            ProviderErrorKind::Other,
            format!(
                "docs demo setup: could not write '{}': {error}",
                full.display()
            ),
            Retryability::Permanent,
        )
    })?;
    Ok(())
}
