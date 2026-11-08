use pretty_assertions::assert_eq;
use rho_protocol as wire;

use super::{available_commands, skill_commands};
use crate::{
    app::session_commands::{test_support::Fixture, BuiltInCommand, CommandSet, Hosting},
    skills::{Skill, SkillSource},
};

// Covers: a menu lists only the built-ins its host runs, and leaves out the
// workflows and agents it refuses, so a host never offers a command it
// rejects (the daemon refuses both until it can run them, #1667, #1670).
// Owner: session options
#[test]
fn a_menu_leaves_out_built_ins_and_kinds_its_host_does_not_run() {
    let fixture = Fixture::new();
    let commands = CommandSet {
        built_ins: &[BuiltInCommand::Init],
        workflows: Hosting::Refused,
        agents: Hosting::Refused,
        sessions_label: "test sessions",
    };

    let listed = available_commands(&fixture.scope(), commands)
        .into_iter()
        .map(|command| (command.name, command.kind, command.input_hint))
        .collect::<Vec<_>>();

    let slash = |name: &str, hint: Option<&str>| {
        (
            name.to_string(),
            wire::CommandKind::Slash,
            hint.map(str::to_string),
        )
    };
    let skill = |name: &str| (name.to_string(), wire::CommandKind::Skill, None);
    assert_eq!(
        listed,
        [
            slash("init", None),
            slash("prompt:review", Some("[$1]")),
            slash("mcp:docs:search", Some("<query>")),
            skill("demo"),
            skill("rho-agent-creator"),
            skill("rho-config"),
            skill("rho-init"),
            skill("rho-workflow-authoring"),
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

// Covers: the skill menu must list exactly the skills a composer can
// mention: T3 Code reads `$` before digits as an amount
// Owner: session options
#[test]
fn skill_commands_skip_names_composers_read_as_amounts() {
    let skills = [
        skill("review"),
        skill("2fa"),
        skill("20k"),
        skill("1e6"),
        skill("42"),
    ];

    let names = skill_commands(&skills)
        .into_iter()
        .map(|command| command.name)
        .collect::<Vec<_>>();

    assert_eq!(names, ["review", "2fa"]);
}
