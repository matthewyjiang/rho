use agent_client_protocol::schema::v1::AvailableCommand;
use pretty_assertions::assert_eq;
use rho_sdk::{model::ContentBlock, UserInput};

use super::{available_commands, first_mention};
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

fn skills() -> Vec<Skill> {
    ["review", "ship-it", "20k", "1e6"]
        .into_iter()
        .map(skill)
        .collect()
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

// Covers: a sent prompt must load the first mention T3 would chip, and only a known skill
// Owner: acp skill commands
#[test]
fn first_mention_follows_t3_mention_rule() {
    let cases: &[(&str, &[&str], Option<&str>)] = &[
        ("leading", &["$review this"], Some("review")),
        ("end of text", &["please $review"], Some("review")),
        ("any currency sigil", &["€ship-it now"], Some("ship-it")),
        (
            "first known wins",
            &["$nope $ship-it $review"],
            Some("ship-it"),
        ),
        (
            "later text block",
            &["no mention", "\t$review"],
            Some("review"),
        ),
        ("needs leading whitespace", &["x$review"], None),
        ("needs trailing whitespace", &["$review."], None),
        ("amount names stay prose", &["pay $20k or $1e6"], None),
        ("unknown skill", &["$missing"], None),
    ];

    for (label, texts, expected) in cases {
        let input = UserInput::content(
            texts
                .iter()
                .map(|text| ContentBlock::Text((*text).into()))
                .collect(),
        )
        .unwrap();
        let mention = first_mention(&input, skills).map(|skill| skill.name);
        assert_eq!(mention.as_deref(), *expected, "{label}");
    }
}

// Covers: prompts without a mention must not walk the skill directories
// Owner: acp skill commands
#[test]
fn first_mention_discovers_only_for_a_mention() {
    let input = UserInput::text("costs $20 today");

    let mention = first_mention(&input, || panic!("discovered without a mention"));

    assert_eq!(mention, None);
}
