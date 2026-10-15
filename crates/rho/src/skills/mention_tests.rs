use pretty_assertions::assert_eq;

use super::skills_to_load;
use crate::skills::{Skill, SkillSource};

fn skills() -> Vec<Skill> {
    ["review", "ship-it", "20k", "1e6"]
        .into_iter()
        .map(|name| Skill {
            name: name.into(),
            description: format!("{name} description"),
            disable_model_invocation: false,
            source: SkillSource::BuiltIn,
            contents: String::new(),
        })
        .collect()
}

/// One case: label, leading skills, prompt texts, and the skills expected to
/// load.
type LoadCase<'a> = (&'a str, &'a [&'a str], &'a [&'a str], &'a [&'a str]);

// Covers: a sent prompt must load every known skill T3 would chip, once each,
// after the skills its command already loads
// Owner: skill mentions
#[test]
fn skills_to_load_follow_t3_mention_rule() {
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
            "command skill loads first",
            &["review"],
            &["$ship-it then $review"],
            &["review", "ship-it"],
        ),
        ("later text", &[], &["no mention", "\t$review"], &["review"]),
        ("needs leading whitespace", &[], &["x$review"], &[]),
        ("needs trailing whitespace", &[], &["$review."], &[]),
        ("amount names stay prose", &[], &["pay $20k or $1e6"], &[]),
        ("unknown skill", &[], &["$missing"], &[]),
    ];

    for (label, leading, texts, expected) in cases {
        assert_eq!(skills_to_load(leading, texts, skills), *expected, "{label}");
    }
}

// Covers: prompts without a mention must not walk the skill directories
// Owner: skill mentions
#[test]
fn skills_to_load_discover_only_for_a_mention() {
    let loaded = skills_to_load(&["review"], &["costs $20 today"], || {
        panic!("discovered without a mention")
    });

    assert_eq!(loaded, ["review"]);
}
