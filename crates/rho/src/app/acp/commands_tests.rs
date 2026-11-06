use agent_client_protocol::schema::v1::{AvailableCommand, AvailableCommandInput};
use pretty_assertions::assert_eq;
use rho_sdk::UserInput;
use serde_json::json;

use super::{available_commands, resolve, skill_commands, AcpPrompt};
use crate::{
    app::session_commands::{test_support::Fixture, PromptCommand},
    skills::{Skill, SkillSource},
};

/// The parts of an ACP prompt the wake tests tell apart.
#[derive(Debug, PartialEq)]
enum Resolved {
    Wake(String),
    Shared(PromptCommand),
    Rejected,
}

// Covers: `/rho-wake <token>` must start a watcher before the shared commands
// see it, reject a token that is not one, and leave a longer word to them
// Owner: acp commands
#[tokio::test]
async fn resolve_matches_the_wake_command_first() {
    let fixture = Fixture::new();
    let scope = fixture.scope();
    let cases = [
        (
            "wake watcher",
            "/rho-wake 0F4E2C3A-9B1D-4E5F-8A7B-6C5D4E3F2A1B",
            Resolved::Wake("0f4e2c3a-9b1d-4e5f-8a7b-6c5d4e3f2a1b".into()),
        ),
        (
            "wake token that could name a path",
            "/rho-wake ../config.toml",
            Resolved::Rejected,
        ),
        (
            "wake prefix of a longer word",
            "/rho-wakeup now",
            Resolved::Shared(PromptCommand::Turn {
                input: UserInput::text("/rho-wakeup now"),
                calls: Vec::new(),
            }),
        ),
    ];

    for (label, text, expected) in cases {
        let resolved = match resolve(UserInput::text(text), &scope).await {
            Ok(AcpPrompt::AwaitWake(signal)) => Resolved::Wake(signal.token().to_owned()),
            Ok(AcpPrompt::Shared(command)) => Resolved::Shared(command),
            Err(_) => Resolved::Rejected,
        };
        assert_eq!(resolved, expected, "{label}");
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
            json!(["$demo", null]),
            json!(["$rho-agent-creator", null]),
            json!(["$rho-config", null]),
            json!(["$rho-init", null]),
            json!(["$rho-workflow-authoring", null]),
            json!(["agent:child-rho", "<task>"]),
            json!(["agent:default", "<task>"]),
            json!(["agent:explorer", "<task>"]),
            json!(["agent:reviewer", "<task>"]),
            json!(["agent:worker", "<task>"]),
        ]
    );
}

fn skill(name: &str) -> Skill {
    Skill {
        name: name.into(),
        description: format!("{name} description"),
        disable_model_invocation: false,
        source: SkillSource::BuiltIn,
        contents: String::new(),
    }
}

// Covers: the `$` menu must list exactly the skills a T3 composer can mention
// Owner: acp commands
#[test]
fn skill_commands_skip_names_t3_reads_as_amounts() {
    let skills = [
        skill("review"),
        skill("2fa"),
        skill("20k"),
        skill("1e6"),
        skill("42"),
    ];

    let names = skill_commands(&skills)
        .into_iter()
        .map(|AvailableCommand { name, .. }| name)
        .collect::<Vec<_>>();

    assert_eq!(names, ["$review", "$2fa"]);
}
