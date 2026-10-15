use agent_client_protocol::schema::v1::AvailableCommand;
use pretty_assertions::assert_eq;
use rho_sdk::{model::ContentBlock, UserInput};

use super::{available_commands, load_calls, SKILL_TOOL};
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

/// Skill names the calls load, in batch order.
fn loaded(leading: &[&str], texts: &[&str]) -> Vec<String> {
    let input = UserInput::content(
        texts
            .iter()
            .map(|text| ContentBlock::Text((*text).into()))
            .collect(),
    )
    .unwrap();
    load_calls(leading, &input, skills)
        .into_iter()
        .map(|call| {
            assert_eq!(call.name, SKILL_TOOL);
            call.arguments["name"].as_str().unwrap().to_owned()
        })
        .collect()
}

/// One `load_calls` case: label, leading skills, prompt text blocks, and the
/// skills expected to load.
type LoadCase<'a> = (&'a str, &'a [&'a str], &'a [&'a str], &'a [&'a str]);

// Covers: a sent prompt must load every known skill T3 would chip, once each,
// after the skill picked as a command
// Owner: acp skill commands
#[test]
fn load_calls_follow_t3_mention_rule() {
    let cases: &[LoadCase] = &[
        ("leading", &[], &["$review this"], &["review"]),
        ("end of text", &[], &["please $review"], &["review"]),
        ("any currency sigil", &[], &["€ship-it now"], &["ship-it"]),
        (
            "every known mention in order",
            &[],
            &["$nope $ship-it $review"],
            &["ship-it", "review"],
        ),
        (
            "each skill once",
            &[],
            &["$review and $review"],
            &["review"],
        ),
        (
            "picked command skill loads first",
            &["review"],
            &["$ship-it then $review"],
            &["review", "ship-it"],
        ),
        (
            "later text block",
            &[],
            &["no mention", "\t$review"],
            &["review"],
        ),
        ("needs leading whitespace", &[], &["x$review"], &[]),
        ("needs trailing whitespace", &[], &["$review."], &[]),
        ("amount names stay prose", &[], &["pay $20k or $1e6"], &[]),
        ("unknown skill", &[], &["$missing"], &[]),
    ];

    for (label, leading, texts, expected) in cases {
        assert_eq!(loaded(leading, texts), *expected, "{label}");
    }
}

// Covers: prompts without a mention must not walk the skill directories
// Owner: acp skill commands
#[test]
fn load_calls_discover_only_for_a_mention() {
    let input = UserInput::text("costs $20 today");

    let calls = load_calls(&["review"], &input, || {
        panic!("discovered without a mention")
    });

    assert_eq!(calls.len(), 1);
}
