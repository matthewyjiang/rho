use rho_sdk::{
    tool::{
        Tool, ToolAccessMode, ToolExecutionPolicy, ToolInvocation, ToolPreparationContext,
        ToolResourceKind,
    },
    CancellationToken, ToolCallId, Workspace,
};
use serde_json::json;
use tempfile::TempDir;

use super::{process, rho, sdk_features, web};

fn invocation(arguments: serde_json::Value) -> ToolInvocation {
    ToolInvocation::new(ToolCallId::from_string("call-1").unwrap(), arguments)
}

fn preparation_context(workspace: Option<Workspace>) -> ToolPreparationContext {
    ToolPreparationContext::new(workspace, CancellationToken::new())
}

fn one_access(
    prepared: &rho_sdk::tool::PreparedToolInvocation<'_>,
) -> (ToolResourceKind, ToolAccessMode) {
    let ToolExecutionPolicy::ResourceAware { accesses } = prepared.execution_policy() else {
        panic!("expected a resource-aware invocation");
    };
    assert_eq!(accesses.len(), 1);
    (accesses[0].resource().kind(), accesses[0].mode())
}

#[tokio::test]
async fn process_actions_prepare_with_audited_policies() {
    let manager = process::ProcessManager::new(process::ProcessLimits::default());
    let tool = process::sdk_process::SdkProcess::new(process::Process::new(manager), 12_000);

    let start = tool
        .prepare(
            invocation(json!({"action": "start", "command": "echo test"})),
            preparation_context(None),
        )
        .await
        .unwrap();
    assert!(matches!(
        start.execution_policy(),
        ToolExecutionPolicy::Exclusive
    ));

    let poll = tool
        .prepare(
            invocation(json!({"action": "poll", "process_id": "process-1"})),
            preparation_context(None),
        )
        .await
        .unwrap();
    assert_eq!(
        one_access(&poll),
        (ToolResourceKind::ManagedProcess, ToolAccessMode::Shared)
    );

    let stop = tool
        .prepare(
            invocation(json!({"action": "stop", "process_id": "process-1"})),
            preparation_context(None),
        )
        .await
        .unwrap();
    assert_eq!(
        one_access(&stop),
        (ToolResourceKind::ManagedProcess, ToolAccessMode::Exclusive)
    );
}

#[tokio::test]
async fn skill_prepares_builtin_and_file_resources() {
    let tool = sdk_features::SdkSkillTool::new(12_000);
    let root = TempDir::new().unwrap();
    let workspace = Workspace::new(root.path()).unwrap();

    let builtin = tool
        .prepare(
            invocation(json!({"name": "rho-diagnostics"})),
            preparation_context(Some(workspace.clone())),
        )
        .await
        .unwrap();
    assert_eq!(
        one_access(&builtin),
        (ToolResourceKind::Opaque, ToolAccessMode::Shared)
    );
    assert_eq!(builtin.capabilities().len(), 1);

    let skill_dir = root.path().join(".agents/skills/prepared-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: prepared-skill\ndescription: prepared test\n---\ncontents\n",
    )
    .unwrap();
    let file = tool
        .prepare(
            invocation(json!({"name": "prepared-skill"})),
            preparation_context(Some(workspace)),
        )
        .await
        .unwrap();
    assert_eq!(
        one_access(&file),
        (ToolResourceKind::WorkspacePath, ToolAccessMode::Shared)
    );
    assert_eq!(file.capabilities().len(), 1);
}

#[tokio::test]
async fn preparation_rejects_invalid_resource_keys_and_actions() {
    let manager = process::ProcessManager::new(process::ProcessLimits::default());
    let process = process::sdk_process::SdkProcess::new(process::Process::new(manager), 12_000);
    let process_error = process
        .prepare(
            invocation(json!({
                "action": "poll",
                "process_id": "process-1",
                "wait_seconds": 31
            })),
            preparation_context(None),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        process_error.kind(),
        rho_sdk::tool::ToolErrorKind::InvalidArguments
    );

    let diagnostics = rho::SdkRho::new(crate::diagnostics::test_diagnostics("openai", "gpt-test"));
    let diagnostics_error = diagnostics
        .prepare(
            invocation(json!({"action": "mutate"})),
            preparation_context(None),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        diagnostics_error.kind(),
        rho_sdk::tool::ToolErrorKind::InvalidArguments
    );

    let get_search_content = web::sdk_get_search_content::SdkGetSearchContent::new(12_000);
    let response_error = get_search_content
        .prepare(
            invocation(json!({"responseId": "../not-a-response"})),
            preparation_context(None),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        response_error.kind(),
        rho_sdk::tool::ToolErrorKind::InvalidArguments
    );
}

#[tokio::test]
async fn diagnostics_and_response_store_prepare_as_safe_reads() {
    let diagnostics = rho::SdkRho::new(crate::diagnostics::test_diagnostics("openai", "gpt-test"));
    let prepared = diagnostics
        .prepare(
            invocation(json!({"action": "info"})),
            preparation_context(None),
        )
        .await
        .unwrap();
    let ToolExecutionPolicy::ResourceAware { accesses } = prepared.execution_policy() else {
        panic!("expected resource-aware diagnostics");
    };
    assert!(accesses.is_empty());

    let get_search_content = web::sdk_get_search_content::SdkGetSearchContent::new(12_000);
    let stored = get_search_content
        .prepare(
            invocation(json!({"responseId": "0123456789abcdef0123456789abcdef"})),
            preparation_context(None),
        )
        .await
        .unwrap();
    assert_eq!(
        one_access(&stored),
        (ToolResourceKind::ResponseStore, ToolAccessMode::Shared)
    );
}
