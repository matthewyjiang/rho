use rho_sdk::{
    tool::{Tool, ToolErrorKind, ToolInvocation, ToolPreparationContext},
    CancellationToken, CapabilityKind, CapabilityOperation, CapabilityRequest, ToolCallId,
    Workspace,
};
use serde_json::json;
use tempfile::TempDir;

use super::SaveAgentTool;
use crate::agent::{persist_destination_path, AgentSaveLocation};
use crate::workspace::ProjectTrust;

fn invocation(arguments: serde_json::Value) -> ToolInvocation {
    ToolInvocation::new(ToolCallId::from_string("call-1").unwrap(), arguments)
}

// Covers: save_agent must reject invalid drafts at prepare, before any write.
// Owner: save_agent tool
#[tokio::test]
async fn prepare_rejects_invalid_definition() {
    let root = TempDir::new().unwrap();
    let tool = SaveAgentTool {
        max_output_bytes: 12_000,
    };
    let result = tool
        .prepare(
            invocation(json!({
                "location": "rho-home",
                "contents": "---\nid: bad\n---\n"
            })),
            ToolPreparationContext::new(
                Some(Workspace::new(root.path()).unwrap()),
                CancellationToken::new(),
            ),
        )
        .await;
    let Err(error) = result else {
        panic!("expected invalid definition to fail prepare");
    };
    assert_eq!(error.kind(), ToolErrorKind::InvalidArguments);
}

fn valid_draft(id: &str) -> String {
    format!(
        "---\nid: {id}\ndescription: save_agent fixture\nprompt: extend\n---\nYou are a fixture.\n"
    )
}

// Covers: execute always reads the destination; prepare must request Read even
// when the file is missing, or a create-after-prepare race discloses it.
// Owner: save_agent tool
#[tokio::test]
async fn prepare_requests_read_when_destination_is_missing() {
    let root = TempDir::new().unwrap();
    let id = "saveagentreadcap";
    let dest = persist_destination_path(
        AgentSaveLocation::RhoHome,
        root.path(),
        crate::paths::home_dir().as_deref(),
        ProjectTrust::Untrusted,
        id,
    )
    .unwrap();
    let tool = SaveAgentTool {
        max_output_bytes: 12_000,
    };
    let prepared = tool
        .prepare(
            invocation(json!({
                "location": "rho-home",
                "contents": valid_draft(id)
            })),
            ToolPreparationContext::new(
                Some(Workspace::new(root.path()).unwrap()),
                CancellationToken::new(),
            ),
        )
        .await
        .unwrap();
    let kinds = prepared
        .capabilities()
        .iter()
        .map(CapabilityRequest::kind)
        .collect::<Vec<_>>();
    assert!(kinds.contains(&CapabilityKind::Write));
    assert!(kinds.contains(&CapabilityKind::Read));
    assert!(prepared.capabilities().iter().any(|request| {
        matches!(
            request.operation(),
            CapabilityOperation::ReadPath { path, .. } if path == &dest
        )
    }));
}
