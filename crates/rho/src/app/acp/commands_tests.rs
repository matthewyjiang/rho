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

/// A workspace with one prompt template and one project skill, an empty
/// home, and a session that can load skills and write files.
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
        let config = Config::default();
        let tools = AppToolSet::new(
            &config,
            RuntimeDiagnostics::new(&config),
            ToolSetOptions::new(AgentCapabilities::new(
                [
                    ToolCapability::Skill,
                    ToolCapability::WriteFile,
                    ToolCapability::Edit,
                ]
                .into(),
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
            home: tempfile::tempdir().unwrap(),
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

/// The parts of an action a test can predict: model text, and the skill a
/// host call loads. Call ids are random.
#[derive(Debug, PartialEq)]
enum Resolved {
    Turn { text: String, skill: Option<String> },
    Compact(Option<String>),
    Rejected,
}

async fn resolved(text: &str, scope: &CommandScope<'_>) -> Resolved {
    match resolve(UserInput::text(text), scope).await {
        Ok(PromptAction::Turn { input, call }) => Resolved::Turn {
            text: match input.blocks() {
                [ContentBlock::Text(text)] => text.clone(),
                blocks => panic!("unexpected blocks {blocks:?}"),
            },
            skill: call.map(|call| {
                assert_eq!(call.name, "skill");
                call.arguments["name"].as_str().unwrap().to_string()
            }),
        },
        Ok(PromptAction::Compact { instructions }) => Resolved::Compact(instructions),
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

fn turn(text: &str, skill: Option<&str>) -> Resolved {
    Resolved::Turn {
        text: text.into(),
        skill: skill.map(str::to_owned),
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
            turn(&init_prompt(fixture.workspace.path()), Some("rho-init")),
        ),
        (
            "prompt template",
            "/prompt:review src/lib.rs",
            turn("Review src/lib.rs carefully", None),
        ),
        (
            "template expansion loads a mentioned skill",
            "/prompt:review $acp-demo",
            turn("Review $acp-demo carefully", Some("acp-demo")),
        ),
        (
            "skill picked as a slash command",
            "/$acp-demo please",
            turn("/$acp-demo please", Some("acp-demo")),
        ),
        (
            "skill mention",
            "use $acp-demo please",
            turn("use $acp-demo please", Some("acp-demo")),
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
            turn("/prompt:missing x", None),
        ),
        (
            "path-like text passes through",
            "/usr/bin/env is odd",
            turn("/usr/bin/env is odd", None),
        ),
    ];

    for (label, text, expected) in cases {
        assert_eq!(resolved(text, &scope).await, expected, "{label}");
    }
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
    let input = UserInput::content(vec![
        ContentBlock::Text("/prompt:review this".into()),
        image.clone(),
    ])
    .unwrap();

    let action = resolve(input, &fixture.scope()).await.unwrap();

    assert_eq!(
        action,
        PromptAction::Turn {
            input: UserInput::content(vec![
                ContentBlock::Text("Review this carefully".into()),
                image,
            ])
            .unwrap(),
            call: None,
        }
    );
}

// Covers: the host menu must list every runnable command with its argument hint
// Owner: acp commands
#[test]
fn available_commands_list_built_ins_templates_mcp_prompts_and_skills() {
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
            json!(["$acp-demo", null]),
            json!(["$rho-agent-creator", null]),
            json!(["$rho-config", null]),
            json!(["$rho-init", null]),
            json!(["$rho-workflow-authoring", null]),
        ]
    );
}
