//! Compare real provider requests across an agent edit, not editor chrome.

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
        "fixture agent config updated" => verify(request),
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

fn verify(request: &ModelRequest<'_>) -> anyhow::Result<ModelResponse> {
    let snapshot: Snapshot = serde_json::from_slice(&std::fs::read(SNAPSHOT_PATH)?)?;
    ensure!(
        request.tools == snapshot.tools,
        "saving the agent changed the original tool schemas"
    );
    ensure!(
        request.messages.starts_with(&snapshot.messages),
        "saving the agent rewrote prior model history"
    );
    let appended = &request.messages[snapshot.messages.len()..];
    ensure!(
        appended
            .first()
            .and_then(Message::completed_assistant_content)
            == Some(&[ContentBlock::Text(BASELINE_RESPONSE.into())][..]),
        "baseline assistant response was not preserved"
    );
    let home = std::env::var_os("HOME").context("isolated HOME missing")?;
    let definition = std::fs::read_to_string(
        std::path::PathBuf::from(home).join(".rho/agents/editable-fixture.md"),
    )?;
    // Match the canonical file payload, not host instructional prose. A literal
    // System entry would be hoisted ahead of history by provider adapters.
    ensure!(
        matches!(appended, [_, Message::User(context), Message::User(_)]
            if context.iter().any(|block| matches!(block, ContentBlock::Text(text)
                if text.contains(&definition)))),
        "next request did not append the saved canonical definition as host context"
    );
    completed("agent config updated: saved definition visible; schemas and history unchanged")
        .map_err(Into::into)
}
