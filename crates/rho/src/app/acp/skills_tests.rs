use agent_client_protocol::schema::v1::AvailableCommand;
use pretty_assertions::assert_eq;

use super::available_commands;
use crate::skills::{Skill, SkillSource};

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
// Owner: acp skill commands
#[test]
fn available_commands_skip_names_t3_reads_as_amounts() {
    let skills = [
        skill("review"),
        skill("2fa"),
        skill("20k"),
        skill("1e6"),
        skill("42"),
    ];

    let names = available_commands(&skills)
        .into_iter()
        .map(|AvailableCommand { name, .. }| name)
        .collect::<Vec<_>>();

    assert_eq!(names, ["$review", "$2fa"]);
}
