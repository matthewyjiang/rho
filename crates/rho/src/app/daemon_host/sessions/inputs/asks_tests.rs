use pretty_assertions::assert_eq;
use rho_protocol::{DefaultAnswer, DefaultStyle};
use rho_sdk::{DefaultSelection, HostChoice, HostQuestion, SelectionMode};
use serde_json::{json, Value};

use super::default_answer;

fn values(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

// Covers: the answer an agent suggests, in whatever JSON it gave, reaches
// clients as the values an answer would send: a choice named by value or by
// label is its value, text naming no choice stays as is, and nothing usable
// is no default; the style says whether to pre-select it.
// Owner: daemon input request projection.
#[test]
fn a_suggested_answer_reads_as_the_values_an_answer_sends() {
    let cases: [(Value, DefaultSelection, Option<DefaultAnswer>); 6] = [
        (
            json!("fast"),
            DefaultSelection::Selected,
            Some(DefaultAnswer {
                values: values(&["fast"]),
                style: DefaultStyle::Selected,
            }),
        ),
        (
            json!("Slow and careful"),
            DefaultSelection::Focused,
            Some(DefaultAnswer {
                values: values(&["slow"]),
                style: DefaultStyle::Focused,
            }),
        ),
        (
            json!(["fast", "Slow and careful", "something else"]),
            DefaultSelection::Selected,
            Some(DefaultAnswer {
                values: values(&["fast", "slow", "something else"]),
                style: DefaultStyle::Selected,
            }),
        ),
        (
            json!(3),
            DefaultSelection::Selected,
            Some(DefaultAnswer {
                values: values(&["3"]),
                style: DefaultStyle::Selected,
            }),
        ),
        (json!(""), DefaultSelection::Selected, None),
        (json!({"pick": "fast"}), DefaultSelection::Selected, None),
    ];
    for (default, selection, expected) in cases {
        let question = HostQuestion::new(
            "speed",
            "how fast?",
            vec![
                HostChoice::new("fast", "Fast"),
                HostChoice::new("slow", "Slow and careful"),
            ],
            SelectionMode::Many,
        )
        .unwrap()
        .with_default_value(default.clone())
        .with_default_selection(selection);

        assert_eq!(default_answer(&question), expected, "default {default}");
    }
}
