//! Compare real provider requests across agent catalog changes, not editor chrome.

use anyhow::{ensure, Context};
use rho_sdk::{
    model::{ContentBlock, Message, ModelRequest, ModelResponse, ToolSpec},
    ProviderError, ProviderErrorKind, Retryability,
};
use serde::{Deserialize, Serialize};

use super::completed;

const SNAPSHOT_PATH: &str = ".rho-fixture-agent-config-request.json";
const BASELINE_RESPONSE: &str = "agent config baseline captured";

#[derive(Serialize, Deserialize)]
struct Snapshot {
    messages: Vec<Message>,
    tools: Vec<ToolSpec>,
}

pub(super) fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
) -> Option<Result<ModelResponse, ProviderError>> {
    let result = match prompt {
        "fixture agent config baseline" => capture(request),
        "fixture agent config updated" => verify(
            request,
            prompt,
            &[serde_json::json!({
                "change": "description",
                "agent_id": "editable-fixture",
                "previous": "fixture agent",
                "description": "fixture agent updated",
            })],
        ),
        "fixture agent config tools updated" => verify(request, prompt, &[]),
        "fixture agent config deleted" => verify(
            request,
            prompt,
            &[serde_json::json!({
                "change": "unavailable",
                "agent_id": "editable-fixture",
            })],
        ),
        _ => return None,
    };
    Some(result.map_err(|error| {
        ProviderError::new(
            ProviderErrorKind::Other,
            format!("agent config fixture: {error:#}"),
            Retryability::Permanent,
        )
    }))
}

fn capture(request: &ModelRequest<'_>) -> anyhow::Result<ModelResponse> {
    ensure!(
        request
            .messages
            .iter()
            .any(|message| matches!(message, Message::ToolResult(_))),
        "baseline must include prior tool history"
    );
    ensure!(
        request.tools.iter().any(|tool| tool.name == "agent"),
        "baseline must include the original agent schema"
    );
    let snapshot = Snapshot {
        messages: request.messages.to_vec(),
        tools: request.tools.to_vec(),
    };
    std::fs::write(SNAPSHOT_PATH, serde_json::to_vec(&snapshot)?)?;
    Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
        BASELINE_RESPONSE.into(),
    )]))
}

fn verify(
    request: &ModelRequest<'_>,
    prompt: &str,
    expected_changes: &[serde_json::Value],
) -> anyhow::Result<ModelResponse> {
    let snapshot: Snapshot = serde_json::from_slice(&std::fs::read(SNAPSHOT_PATH)?)?;
    ensure!(
        request.tools == snapshot.tools,
        "changing the agent catalog changed the original tool schemas"
    );
    ensure!(
        request.messages.starts_with(&snapshot.messages),
        "changing the agent catalog rewrote prior model history"
    );
    let appended = &request.messages[snapshot.messages.len()..];
    ensure!(
        appended
            .first()
            .and_then(Message::completed_assistant_content)
            == Some(&[ContentBlock::Text(BASELINE_RESPONSE.into())][..]),
        "baseline assistant response was not preserved"
    );
    ensure!(
        appended.last() == Some(&Message::user_text(prompt)),
        "next request did not preserve the verification prompt"
    );
    let notices = &appended[1..appended.len() - 1];
    if expected_changes.is_empty() {
        ensure!(
            notices.is_empty(),
            "tools-only edit appended unexpected catalog context: {notices:?}"
        );
    } else {
        // User-role framing keeps corrections after history instead of hoisting
        // them into the original system prompt in provider adapters.
        let [Message::User(content)] = notices else {
            anyhow::bail!("expected one appended catalog notice, got {notices:?}");
        };
        let [ContentBlock::Text(text)] = content.as_slice() else {
            anyhow::bail!("catalog notice must contain only its text payload");
        };
        let payload = text
            .strip_prefix("[agent catalog updated]\n")
            .context("catalog notice framing missing")?;
        let changes: Vec<serde_json::Value> = serde_json::from_str(payload)?;
        ensure!(
            changes == expected_changes,
            "catalog notice mismatch: expected {expected_changes:?}, got {changes:?}"
        );
    }
    completed("agent config verified: schemas and history unchanged").map_err(Into::into)
}
