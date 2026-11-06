use pretty_assertions::assert_eq;
use rho_sdk::{model::ContentBlock, UserInput};
use serde_json::json;

use super::{
    resolve, test_support::Fixture, BuiltInCommand, CommandScope, CommandSet, PromptCommand,
};
use crate::app::session_runtime::AgentTask;

/// A host that runs every built-in.
const ALL: CommandSet = CommandSet {
    built_ins: &[BuiltInCommand::Compact, BuiltInCommand::Init],
    sessions_label: "test sessions",
};

/// The parts of a command a test can predict: model text, and the skills its
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
        model_override: Option<String>,
        input: UserInput,
    },
    Rejected,
}

async fn resolved(text: &str, scope: &CommandScope<'_>, commands: CommandSet) -> Resolved {
    match resolve(UserInput::text(text), scope, commands).await {
        Ok(PromptCommand::Turn { input, calls }) => Resolved::Turn {
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
        Ok(PromptCommand::Compact { instructions }) => Resolved::Compact(instructions),
        Ok(PromptCommand::StartWorkflow(start)) => Resolved::Workflow {
            file: start.workflow.relative_path,
            inputs: json!(start.inputs),
        },
        Ok(PromptCommand::RunAsAgent(AgentTask {
            agent_id,
            model_override,
            input,
        })) => Resolved::Agent {
            id: agent_id,
            model_override,
            input,
        },
        Err(_) => Resolved::Rejected,
    }
}

fn init_prompt(workspace: &std::path::Path) -> String {
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
// Owner: session commands
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
            turn(&init_prompt(fixture.workspace()), &["rho-init"]),
        ),
        (
            "prompt template",
            "/prompt:review src/lib.rs",
            turn("Review src/lib.rs carefully", &[]),
        ),
        (
            "template expansion loads a mentioned skill",
            "/prompt:review $demo",
            turn("Review $demo carefully", &["demo"]),
        ),
        (
            "skill picked as a slash command",
            "/$demo please",
            turn("/$demo please", &["demo"]),
        ),
        (
            "slash command skill batches with later mentions",
            "/$demo with $rho-config and $demo",
            turn("/$demo with $rho-config and $demo", &["demo", "rho-config"]),
        ),
        (
            "skill mention",
            "use $demo please",
            turn("use $demo please", &["demo"]),
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
            "/agent:child-rho inspect the parser",
            Resolved::Agent {
                id: "child-rho".into(),
                model_override: None,
                input: UserInput::text("inspect the parser"),
            },
        ),
        ("agent missing task", "/agent:child-rho", Resolved::Rejected),
        (
            "multiline delegated brief",
            "/agent:child-rho\n\nGoal: inspect the parser.\n\nReport findings.\n",
            Resolved::Agent {
                id: "child-rho".into(),
                model_override: None,
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
            "/agent:child-claude inspect",
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
        assert_eq!(resolved(text, &scope, ALL).await, expected, "{label}");
    }
    let plan = CommandScope {
        permission_mode: crate::permission::PermissionMode::Plan,
        ..fixture.scope()
    };
    assert_eq!(
        resolved("/workflow:check", &plan, ALL).await,
        Resolved::Rejected,
        "plan mode cannot start workflows"
    );
    // A host such as `rho run`, whose one prompt has nothing to compact.
    let without_compact = CommandSet {
        built_ins: &[BuiltInCommand::Init],
        sessions_label: "test runs",
    };
    assert_eq!(
        resolved("/compact", &scope, without_compact).await,
        Resolved::Rejected,
        "a host whose set leaves out /compact rejects it"
    );
}

// Covers: a command must keep the images attached to its prompt
// Owner: session commands
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
            PromptCommand::Turn {
                input: UserInput::content(vec![
                    ContentBlock::Text("Review this carefully".into()),
                    image.clone(),
                ])
                .unwrap(),
                calls: Vec::new(),
            },
        ),
        (
            "/agent:child-rho inspect this",
            PromptCommand::RunAsAgent(AgentTask {
                agent_id: "child-rho".into(),
                model_override: None,
                input: UserInput::content(vec![
                    ContentBlock::Text("inspect this".into()),
                    image.clone(),
                ])
                .unwrap(),
            }),
        ),
        (
            "/agent:child-rho --model @review -- \nInspect this\n--model is task text\n",
            PromptCommand::RunAsAgent(AgentTask {
                agent_id: "child-rho".into(),
                model_override: Some("@review".into()),
                input: UserInput::content(vec![
                    ContentBlock::Text("\nInspect this\n--model is task text\n".into()),
                    image.clone(),
                ])
                .unwrap(),
            }),
        ),
    ] {
        let input =
            UserInput::content(vec![ContentBlock::Text(command.into()), image.clone()]).unwrap();
        assert_eq!(
            resolve(input, &fixture.scope(), ALL).await.unwrap(),
            expected
        );
    }
}

// Covers: hosts must reject ambiguous override syntax rather than treating flags
// as task text, and keep catalog references distinct from the delegated brief.
// Owner: session command parser; catalog resolution belongs to the binder.
#[tokio::test]
async fn agent_model_override_requires_a_reference_and_explicit_task_delimiter() {
    let fixture = Fixture::new();
    for (args, expected) in [
        (
            "-- --model is literal task text",
            Resolved::Agent {
                id: "child-rho".into(),
                model_override: None,
                input: UserInput::text("--model is literal task text"),
            },
        ),
        (
            "--model openai-codex/gpt-6-luna -- inspect --model literally",
            Resolved::Agent {
                id: "child-rho".into(),
                model_override: Some("openai-codex/gpt-6-luna".into()),
                input: UserInput::text("inspect --model literally"),
            },
        ),
        ("--model", Resolved::Rejected),
        ("--model -- inspect", Resolved::Rejected),
        ("--model @review", Resolved::Rejected),
        ("--model @review inspect", Resolved::Rejected),
        ("--model @review --", Resolved::Rejected),
        ("--model @review -- \n\t", Resolved::Rejected),
        ("--model @review --model @other -- task", Resolved::Rejected),
        ("--model @two words -- task", Resolved::Rejected),
        ("--model=@review -- task", Resolved::Rejected),
    ] {
        assert_eq!(
            resolved(&format!("/agent:child-rho {args}"), &fixture.scope(), ALL).await,
            expected,
            "{args}"
        );
    }
}
