use std::{io, sync::Arc};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, Tool, ToolOutput},
    SessionOptions, SystemPrompt, Workspace,
};
use serde_json::json;

use super::{
    complete_run, prompt_from_reader, wait_for_cancel_request, AutomationWorkspacePolicy,
    RunReporter, SubagentCancelled,
};
use crate::{
    app::runtime_builder::{build_runtime, RuntimeBuildOptions},
    compaction::CompactionConfig,
};

#[test]
fn prompt_joins_inline_parts() {
    let mut stdin = io::empty();
    let prompt = prompt_from_reader(
        vec!["review".into(), "this".into()],
        /*read_stdin*/ false,
        &mut stdin,
    )
    .unwrap();

    assert_eq!(prompt, "review this");
}

#[test]
fn prompt_combines_inline_and_stdin() {
    let mut stdin = "diff contents".as_bytes();
    let prompt =
        prompt_from_reader(vec!["review".into()], /*read_stdin*/ true, &mut stdin).unwrap();

    assert_eq!(prompt, "review\n\ndiff contents");
}

#[test]
fn prompt_requires_input() {
    let mut stdin = io::empty();
    let error = prompt_from_reader(Vec::new(), /*read_stdin*/ false, &mut stdin).unwrap_err();

    assert!(error.to_string().contains("requires a prompt"));
}

#[tokio::test]
async fn cancel_marker_finalizes_a_stopped_partial_result() {
    let dir = tempfile::tempdir().unwrap();
    let output_file = dir.path().join(crate::subagent::RESULT_FILE_NAME);
    let mut reporter = RunReporter::new(output_file.clone(), Some("worker".into())).unwrap();
    reporter.status.last_text = Some("work in progress".into());

    crate::subagent::request_cancel(&output_file).unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        wait_for_cancel_request(Some(reporter.cancel_file.clone())),
    )
    .await
    .expect("cancel marker was not observed")
    .unwrap();
    reporter.finish(&Err(SubagentCancelled.into()));

    let status = crate::subagent::read_status(&output_file).unwrap();
    assert_eq!(status.state, crate::subagent::RunState::Stopped);
    assert_eq!(
        status.result.as_deref(),
        Some("(partial, stopped before finishing)\nwork in progress")
    );
}

#[tokio::test]
async fn reporter_clears_a_stale_cancel_marker() {
    let dir = tempfile::tempdir().unwrap();
    let output_file = dir.path().join(crate::subagent::RESULT_FILE_NAME);
    crate::subagent::request_cancel(&output_file).unwrap();

    let reporter = RunReporter::new(output_file, Some("worker".into())).unwrap();

    assert!(!reporter.cancel_file.exists());
}

#[tokio::test]
async fn headless_run_compacts_at_configured_threshold_and_completes() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "automation-compaction"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "call-1".into(),
                    name: "expand_context".into(),
                    arguments: json!({}),
                },
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "compact summary".into(),
            )])),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let shared_provider: Arc<dyn ModelProvider> = Arc::new(provider.clone());
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(ScriptedTool::new(
        ToolSpec {
            name: "expand_context".into(),
            description: "return a large result".into(),
            input_schema: json!({"type": "object"}),
        },
        ScriptedToolOutcome::Success(ToolOutput::text("tool context ".repeat(500))),
    ))];
    let root = tempfile::tempdir().unwrap();
    let runtime = build_runtime(RuntimeBuildOptions {
        provider: shared_provider,
        tools: &tools,
        workspace: Workspace::new(root.path()).unwrap(),
        workspace_policy: AutomationWorkspacePolicy,
        system_prompt: SystemPrompt::None,
        reasoning: rho_sdk::ReasoningLevel::Off,
        compaction: CompactionConfig {
            auto_compact: true,
            threshold_percent: 5,
            target_percent: 1,
        },
        context_window: Some(1_000),
    })
    .unwrap();
    assert_eq!(runtime.diagnostics().compaction_trigger_tokens(), Some(50));
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    let outcome = complete_run(&session, "continue".into(), None)
        .await
        .unwrap();

    assert_eq!(outcome.text(), "done");
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].messages.iter().any(|message| {
        matches!(
            message,
            Message::User(blocks)
                if blocks.iter().any(|block| matches!(
                    block,
                    ContentBlock::Text(text)
                        if text.starts_with("Automatic compaction summary")
                ))
        )
    }));
    runtime.shutdown();
}
