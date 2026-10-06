use std::time::Duration;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelResponse, ToolCall},
    provider::ScriptedTurn,
    HostInputResponse,
};
use serde_json::json;

use super::*;
use crate::{
    agent::{AgentCapabilities, ToolCapability},
    tools::{plan_exit::PlanExitDecision, sdk_registry::ToolSetOptions},
};

// Covers: plan approval is consumable once and does not swap policy during the SDK turn;
// the next runtime advertises handoff only in Plan, only with an interactive question host.
// Owner: interactive runtime tool registration and turn boundary.
#[tokio::test]
async fn plan_exit_registration_and_one_shot_decision() {
    let mut agent = super::super::test_runtime(vec![
        ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
            ToolCall {
                id: "plan-exit-test".into(),
                name: "exit_plan_mode".into(),
                arguments: json!({"plan":"# Plan\nImplement the change"}),
            },
        )])),
        ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
            "acknowledged".into(),
        )])),
    ])
    .await;
    let config = agent.config_snapshot();
    for (capabilities, interactive_host, available) in [
        (vec![ToolCapability::Questionnaire], false, false),
        (vec![], true, false),
        (vec![ToolCapability::Questionnaire], true, true),
    ] {
        let tools = AppToolSet::new(
            &config,
            agent.diagnostics.clone(),
            ToolSetOptions::new(AgentCapabilities::new(capabilities.into_iter().collect())),
        );
        agent.tools = if interactive_host {
            tools.with_plan_exit_host(&config)
        } else {
            tools
        };
        for mode in [
            PermissionMode::Plan,
            PermissionMode::Supervised,
            PermissionMode::Plan,
        ] {
            agent.set_permission_mode(mode).await.unwrap();
            assert_eq!(
                agent.has_tool("exit_plan_mode"),
                available && mode == PermissionMode::Plan
            );
        }
        agent
            .set_permission_mode(PermissionMode::Bypass)
            .await
            .unwrap();
    }
    agent
        .set_permission_mode(PermissionMode::Plan)
        .await
        .unwrap();
    // The bound covers a missing host-input event; synchronization is the event itself.
    tokio::time::timeout(Duration::from_secs(10), async {
        agent
            .start(UserInput::text("make a plan"), /*display_user*/ None)
            .await
            .unwrap();
        while let Some(event) = agent.next_event().await {
            if let RunEvent::ToolHostInputRequested { request, .. } = event {
                agent
                    .respond(
                        request.id().clone(),
                        HostInputResponse::new().answer("decision", ["allow_edits"]),
                    )
                    .await
                    .unwrap();
            }
        }
        agent.finish_run().await.unwrap();
    })
    .await
    .expect("plan approval turn did not finish");
    assert_eq!(agent.permission_mode(), PermissionMode::Plan);
    assert_eq!(
        agent.take_plan_exit_decision(),
        Some(PlanExitDecision {
            target: PermissionMode::AllowEdits,
            feedback: None
        })
    );
    assert_eq!(agent.take_plan_exit_decision(), None);
    agent
        .set_permission_mode(PermissionMode::AllowEdits)
        .await
        .unwrap();
    assert!(!agent.has_tool("exit_plan_mode"));
    agent.shutdown().await;
}
