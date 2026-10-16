use std::path::Path;

use agent_client_protocol::schema::v1::AvailableCommandInput;
use pretty_assertions::assert_eq;
use rho_sdk::{model::ContentBlock, UserInput};
use serde_json::json;

use super::{available_commands, resolve, CommandScope, PromptAction};
use crate::{
    agent::{AgentCapabilities, ToolCapability},
    config::Config,
    diagnostics::RuntimeDiagnostics,
    tools::{
        mcp::catalog::{McpPrompt, McpPromptArgument},
        sdk_registry::{AppToolSet, ToolSetOptions},
    },
};

/// A workspace with one prompt template, one project skill, and one
/// workflow, user definitions for a Rho and an external agent, and a session
/// that can load skills, start workflows, and write files (but not delegate).
struct Fixture {
    workspace: tempfile::TempDir,
    home: tempfile::TempDir,
    tools: AppToolSet,
    config: Config,
}

impl Fixture {
    fn new() -> Self {
        let workspace = tempfile::tempdir().unwrap();
        let prompts = workspace.path().join(".rho/prompts");
        std::fs::create_dir_all(&prompts).unwrap();
        std::fs::write(prompts.join("review.md"), "Review $1 carefully").unwrap();
        let skill = workspace.path().join(".agents/skills/acp-demo");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: acp-demo\ndescription: Demo skill.\n---\nBody\n",
        )
        .unwrap();
        let workflows = workspace.path().join(".rho/workflows/check");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join("workflow.star"), "").unwrap();
        let home = tempfile::tempdir().unwrap();
        let agents = home.path().join(".rho/agents");
        std::fs::create_dir_all(&agents).unwrap();
        for (id, contents) in [
            (
                "acp-rho",
                "---\ndescription: Rho child\ntools: [read_file]\n---\nInspect files\n",
            ),
            (
                "acp-claude",
                "---\ndescription: Claude child\nruntime: claude-cli\ntools: [Read]\n---\nInspect files\n",
            ),
        ] {
            std::fs::write(agents.join(format!("{id}.md")), contents).unwrap();
        }
        let config = Config::default();
        let tools = AppToolSet::new(
            &config,
            RuntimeDiagnostics::new(&config),
            ToolSetOptions::new(AgentCapabilities::new(
                [
                    ToolCapability::Skill,
                    ToolCapability::Workflow,
                    ToolCapability::WriteFile,
                    ToolCapability::Edit,
                ]
                .into(),
            ))
            .workflow(crate::app::workflow_cli::workflow_tool_service(
                workspace.path().to_path_buf(),
                /*config_path*/ None,
                crate::tools::workflow_tracker::WorkflowRunTracker::new(),
            )),
        );
        tools.mcp_catalog().insert_offline_prompt(
            McpPrompt {
                server: "docs".into(),
                name: "search".into(),
                title: None,
                description: Some("Search the docs".into()),
                arguments: vec![McpPromptArgument {
                    name: "query".into(),
                    description: None,
                    required: true,
                }],
            },
            /*completions*/ false,
        );
        Self {
            workspace,
            home,
            tools,
            config,
        }
    }

    fn scope(&self) -> CommandScope<'_> {
        CommandScope {
            cwd: self.workspace.path(),
            home: Some(self.home.path()),
            tools: &self.tools,
            config: &self.config,
            permission_mode: crate::permission::PermissionMode::Supervised,
        }
    }
}

/// The parts of an action a test can predict: model text, and the skills its
/// host batch loads, in order. Call ids are random.
#[derive(Debug, PartialEq)]
enum Resolved {
    Turn {
        text: String,
        skills: Vec<String>,
    },
    Compact(Option<String>),
    Workflow {
        file: String,
        inputs: serde_json::Value,
    },
    Agent {
        id: String,
        input: UserInput,
    },
    Rejected,
}

async fn resolved(text: &str, scope: &CommandScope<'_>) -> Resolved {
    match resolve(UserInput::text(text), scope).await {
        Ok(PromptAction::Turn { input, calls }) => Resolved::Turn {
            text: match input.blocks() {
                [ContentBlock::Text(text)] => text.clone(),
                blocks => panic!("unexpected blocks {blocks:?}"),
            },
            skills: calls
                .into_iter()
                .map(|call| {
                    assert_eq!(call.name, "skill");
                    call.arguments["name"].as_str().unwrap().to_string()
                })
                .collect(),
        },
        Ok(PromptAction::Compact { instructions }) => Resolved::Compact(instructions),
        Ok(PromptAction::StartWorkflow(start)) => Resolved::Workflow {
            file: start.workflow.relative_path,
            inputs: json!(start.inputs),
        },
        Ok(PromptAction::RunAsAgent { agent_id, input }) => Resolved::Agent {
            id: agent_id,
            input,
        },
        Err(_) => Resolved::Rejected,
    }
}

fn init_prompt(workspace: &Path) -> String {
    crate::commands::init_turn::model_prompt(
        workspace,
        crate::permission::PermissionMode::Supervised,
        |_| true,
    )
    .unwrap()
}

fn turn(text: &str, skills: &[&str]) -> Resolved {
    Resolved::Turn {
        text: text.into(),
        skills: skills.iter().map(|skill| (*skill).to_owned()).collect(),
    }
}

fn workflow(inputs: serde_json::Value) -> Resolved {
    Resolved::Workflow {
        file: ".rho/workflows/check/workflow.star".into(),
        inputs,
    }
}

// Covers: each prompt shape must reach its command, and anything else the model unchanged
// Owner: acp commands
#[tokio::test]
async fn resolve_routes_prompts_to_commands() {
    let fixture = Fixture::new();
    let scope = fixture.scope();
    let cases = [
        ("compact", "/compact", Resolved::Compact(None)),
        (
            "compact with instructions",
            "/compact keep the API notes",
            Resolved::Compact(Some("keep the API notes".into())),
        ),
        (
            "init",
            "/init",
            turn(&init_prompt(fixture.workspace.path()), &["rho-init"]),
        ),
        (
            "prompt template",
            "/prompt:review src/lib.rs",
            turn("Review src/lib.rs carefully", &[]),
        ),
        (
            "template expansion loads a mentioned skill",
            "/prompt:review $acp-demo",
            turn("Review $acp-demo carefully", &["acp-demo"]),
        ),
        (
            "skill picked as a slash command",
            "/$acp-demo please",
            turn("/$acp-demo please", &["acp-demo"]),
        ),
        (
            "slash command skill batches with later mentions",
            "/$acp-demo with $rho-config and $acp-demo",
            turn(
                "/$acp-demo with $rho-config and $acp-demo",
                &["acp-demo", "rho-config"],
            ),
        ),
        (
            "skill mention",
            "use $acp-demo please",
            turn("use $acp-demo please", &["acp-demo"]),
        ),
        ("tui-only command", "/model gpt-5", Resolved::Rejected),
        (
            "mcp prompt missing arguments",
            "/mcp:docs:search",
            Resolved::Rejected,
        ),
        (
            "mcp prompt on a disconnected server",
            "/mcp:docs:search rust",
            Resolved::Rejected,
        ),
        (
            "unknown template passes through",
            "/prompt:missing x",
            turn("/prompt:missing x", &[]),
        ),
        (
            "workflow with default inputs",
            "/workflow:check",
            workflow(json!({})),
        ),
        (
            "workflow with inputs",
            r#"/workflow:check {"target": "src"}"#,
            workflow(json!({"target": "src"})),
        ),
        ("unknown workflow", "/workflow:missing", Resolved::Rejected),
        (
            "agent definition",
            "/agent:acp-rho inspect the parser",
            Resolved::Agent {
                id: "acp-rho".into(),
                input: UserInput::text("inspect the parser"),
            },
        ),
        ("agent missing task", "/agent:acp-rho", Resolved::Rejected),
        (
            "multiline delegated brief",
            "/agent:acp-rho\n\nGoal: inspect the parser.\n\nReport findings.\n",
            Resolved::Agent {
                id: "acp-rho".into(),
                input: UserInput::text("Goal: inspect the parser.\n\nReport findings."),
            },
        ),
        ("agent missing name", "/agent: inspect", Resolved::Rejected),
        (
            "unknown agent",
            "/agent:missing inspect",
            Resolved::Rejected,
        ),
        (
            "external agent",
            "/agent:acp-claude inspect",
            Resolved::Rejected,
        ),
        (
            "workflow inputs that are not an object",
            "/workflow:check src",
            Resolved::Rejected,
        ),
        (
            "path-like text passes through",
            "/usr/bin/env is odd",
            turn("/usr/bin/env is odd", &[]),
        ),
    ];

    for (label, text, expected) in cases {
        assert_eq!(resolved(text, &scope).await, expected, "{label}");
    }
    let plan = CommandScope {
        permission_mode: crate::permission::PermissionMode::Plan,
        ..fixture.scope()
    };
    assert_eq!(
        resolved("/workflow:check", &plan).await,
        Resolved::Rejected,
        "plan mode cannot start workflows"
    );
}

// Covers: a command must keep the images attached to its prompt
// Owner: acp commands
#[tokio::test]
async fn resolve_keeps_blocks_after_the_command() {
    let fixture = Fixture::new();
    let image = ContentBlock::Image(rho_sdk::model::ImageContent {
        mime_type: "image/png".into(),
        data: "AAAA".into(),
    });
    for (command, expected) in [
        (
            "/prompt:review this",
            PromptAction::Turn {
                input: UserInput::content(vec![
                    ContentBlock::Text("Review this carefully".into()),
                    image.clone(),
                ])
                .unwrap(),
                calls: Vec::new(),
            },
        ),
        (
            "/agent:acp-rho inspect this",
            PromptAction::RunAsAgent {
                agent_id: "acp-rho".into(),
                input: UserInput::content(vec![
                    ContentBlock::Text("inspect this".into()),
                    image.clone(),
                ])
                .unwrap(),
            },
        ),
    ] {
        let input =
            UserInput::content(vec![ContentBlock::Text(command.into()), image.clone()]).unwrap();
        assert_eq!(resolve(input, &fixture.scope()).await.unwrap(), expected);
    }
}

// Covers: the host menu must list every runnable command with its argument hint
// Owner: acp commands
#[test]
fn available_commands_list_every_command_kind() {
    let fixture = Fixture::new();

    let commands = available_commands(&fixture.scope())
        .into_iter()
        .map(|command| {
            let hint = command.input.map(|input| match input {
                AvailableCommandInput::Unstructured(input) => input.hint,
                _ => unreachable!("rho only sends unstructured input"),
            });
            json!([command.name, hint])
        })
        .collect::<Vec<_>>();

    assert_eq!(
        commands,
        [
            json!(["compact", "[instructions]"]),
            json!(["init", null]),
            json!(["prompt:review", "[$1]"]),
            json!(["mcp:docs:search", "<query>"]),
            json!(["workflow:check", "[inputs JSON]"]),
            json!(["$acp-demo", null]),
            json!(["$rho-agent-creator", null]),
            json!(["$rho-config", null]),
            json!(["$rho-init", null]),
            json!(["$rho-workflow-authoring", null]),
            json!(["agent:acp-rho", "<task>"]),
            json!(["agent:default", "<task>"]),
            json!(["agent:explorer", "<task>"]),
            json!(["agent:reviewer", "<task>"]),
            json!(["agent:worker", "<task>"]),
        ]
    );
}
