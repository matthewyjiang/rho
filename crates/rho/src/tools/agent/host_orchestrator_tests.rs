use pretty_assertions::assert_eq;

use super::*;
use crate::agent::{
    AgentId, AgentRuntimeSpec, ModelPolicy, ModelSelection, PromptPolicy, ToolPolicy,
};

// Covers: `--t3code` binds only the complete tool set from the `t3-code`
// server; a partial connection or another server's tools fail the session
// instead of delegating to the wrong place. Owner: host connection.
#[test]
fn connection_requires_the_complete_named_host_contract() {
    for (server, remote_tools, connects) in [
        (
            "t3-code",
            vec!["delegate_task", "task_status", "task_cancel"],
            true,
        ),
        (
            "another-host",
            vec!["delegate_task", "task_status", "task_cancel"],
            false,
        ),
        ("t3-code", vec!["task_status", "task_cancel"], false),
        ("t3-code", vec!["delegate_task", "task_cancel"], false),
        ("t3-code", vec!["delegate_task", "task_status"], false),
    ] {
        let mut tools = AppToolSet::disabled();
        tools.add_bundle(crate::tools::sdk_registry::StaticToolBundle::new(
            remote_tools
                .iter()
                .map(|name| {
                    Arc::new(rho_sdk::tool::ScriptedTool::new(
                        rho_sdk::model::ToolSpec {
                            name: namespaced_tool_name(server, name),
                            description: String::new(),
                            input_schema: json!({"type": "object"}),
                        },
                        rho_sdk::tool::ScriptedToolOutcome::Success(ToolOutput::text("unused")),
                    )) as Arc<dyn Tool>
                })
                .collect(),
        ));
        assert_eq!(
            HostOrchestrator::connect(&tools).is_ok(),
            connects,
            "{server} {remote_tools:?}"
        );
    }
}

// Covers: T3 children must rebind to their definition, and a pinned model is
// never forwarded to the host, which rejects models Rho does not advertise.
// Owner: host launch policy.
#[test]
fn delegation_arguments_route_through_the_child_rebind() {
    let prompt = "\n  \nImplement the parser\nKeep public APIs stable";
    let definition = AgentDefinition {
        id: AgentId::new("host-worker").unwrap(),
        description: "Implementation".into(),
        prompt: PromptPolicy::Extend("Use the definition's prompt".into()),
        runtime: AgentRuntimeSpec::Rho {
            tools: ToolPolicy::All,
            model: ModelPolicy::Require(ModelSelection {
                provider: Some("openai-codex".into()),
                model: "gpt-6-luna".into(),
                auth: None,
                fast: false,
            }),
            reasoning: None,
        },
    };
    assert_eq!(
        delegate_arguments(&definition, prompt),
        json!({
            "task": format!("/agent:host-worker {prompt}"),
            "title": "Implement the parser",
            "mode": "async",
        })
    );
}
