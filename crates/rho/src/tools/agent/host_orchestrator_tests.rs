use pretty_assertions::assert_eq;
use rho_sdk::{tool::tool_progress_channel, CancellationToken};

use super::*;
use crate::agent::{
    AgentId, AgentRuntimeSpec, ModelPolicy, ModelSelection, PromptPolicy, ToolPolicy,
};

fn context() -> ToolContext {
    let (progress, _receiver) = tool_progress_channel(std::num::NonZeroUsize::MIN);
    ToolContext::new(/*workspace*/ None, CancellationToken::new(), progress)
}

// Covers: `--t3code` binds only the complete tool set from the `t3-code`
// server; with a partial set or another server's tools, connecting still
// succeeds (hosts probe sessions without MCP servers) but every host operation
// fails as an execution error instead of reaching the wrong place.
// Owner: host connection.
#[tokio::test]
async fn host_operations_require_the_complete_named_host_contract() {
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
        ("t3-code", vec![], false),
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
        let orchestrator = HostOrchestrator::connect(&tools);
        let id = || ToolCallId::from_string("host-call").unwrap();
        let expected = if connects {
            Ok(())
        } else {
            Err(ToolErrorKind::Execution)
        };
        for result in [
            orchestrator.status(id(), "task-1", context()).await,
            orchestrator
                .cancel(id(), "task-1", /*reason*/ None, context())
                .await,
        ] {
            assert_eq!(
                result.map(drop).map_err(|error| error.kind()),
                expected,
                "{server} {remote_tools:?}"
            );
        }
    }
}

// Covers: T3 children must rebind to their definition, and a pinned model is
// never forwarded to the host, which rejects models Rho does not advertise;
// only an explicit per-call override reaches the child, through its rebind.
// Owner: host launch policy.
#[test]
fn child_task_routes_through_the_child_rebind() {
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
    let cases = [
        (
            "multi-line prompt",
            "\n  \nImplement the parser",
            /*model_override*/ None,
            "/agent:host-worker \n  \nImplement the parser",
        ),
        (
            "prompt that looks like a flag",
            "--model is literal task text",
            None,
            "/agent:host-worker -- --model is literal task text",
        ),
        (
            "per-call model override",
            "Implement the parser",
            Some("openai/gpt-5"),
            "/agent:host-worker --model openai/gpt-5 -- Implement the parser",
        ),
    ];
    for (label, prompt, model_override, expected) in cases {
        assert_eq!(
            child_task(&definition, prompt, model_override),
            expected,
            "{label}"
        );
    }
}
