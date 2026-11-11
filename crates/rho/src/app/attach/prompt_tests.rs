use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use rho_protocol::{
    ApprovalAsk, ApprovalSource, Ask, Choice, DefaultAnswer, DefaultStyle, InputAnswer,
    InputRequest, InputRequestId, Operation, PermissionMode, PlanApprovalAsk, Question,
    QuestionAsk, Selection,
};

use super::{Prompt, Reply};

fn request(ask: Ask) -> InputRequest {
    InputRequest {
        id: InputRequestId(1),
        ask,
        tool_call: None,
        timeout: None,
        asker: Default::default(),
    }
}

fn approval() -> Ask {
    Ask::Approval(ApprovalAsk {
        source: ApprovalSource::Rho,
        operation: Operation::Network { url: None },
        reason: "fetch the docs".into(),
    })
}

/// A plan approval offering auto, a mode this rho does not know, and
/// allow_edits.
fn plan() -> Ask {
    Ask::PlanApproval(PlanApprovalAsk {
        plan: "1. edit\n2. test\n".into(),
        modes: vec![
            PermissionMode::Auto,
            PermissionMode::Unknown,
            PermissionMode::AllowEdits,
        ],
    })
}

fn choice(value: &str, label: &str, description: Option<&str>) -> Choice {
    Choice {
        value: value.into(),
        label: label.into(),
        description: description.map(Into::into),
    }
}

/// Which database, one of two, required; which features, any of three or
/// the user's own, optional, defaulting to "docs" in `default_style`; and a
/// name, a text field as the questionnaire tool sends one.
fn questions(default_style: DefaultStyle) -> Ask {
    Ask::Question(QuestionAsk {
        title: "Setup".into(),
        questions: vec![
            Question {
                id: "db".into(),
                prompt: "Which database?".into(),
                header: None,
                choices: vec![
                    choice("pg", "postgres", Some("the server")),
                    choice("lite", "sqlite", None),
                ],
                selection: Selection::One,
                allow_other: false,
                help: Some("pick what CI runs".into()),
                default: None,
                required: true,
            },
            Question {
                id: "features".into(),
                prompt: "Which features?".into(),
                header: Some("Features".into()),
                choices: vec![
                    choice("docs", "docs", None),
                    choice("ci", "CI", None),
                    choice("bench", "benchmarks", None),
                ],
                selection: Selection::Many,
                allow_other: true,
                help: None,
                default: Some(DefaultAnswer {
                    values: vec!["docs".into(), "docs".into()],
                    style: default_style,
                }),
                required: false,
            },
            Question {
                id: "name".into(),
                prompt: "Project name?".into(),
                header: None,
                choices: vec![choice("other", "Other", None)],
                selection: Selection::One,
                allow_other: true,
                help: None,
                default: None,
                required: true,
            },
        ],
    })
}

fn answers(pairs: &[(&str, &[&str])]) -> Reply {
    Reply::Answer(InputAnswer::Answers {
        answers: pairs
            .iter()
            .map(|(id, values)| {
                (
                    (*id).to_owned(),
                    values.iter().map(|value| (*value).to_owned()).collect(),
                )
            })
            .collect::<BTreeMap<_, _>>(),
    })
}

fn retry(why: &str) -> Reply {
    Reply::Retry(why.to_owned())
}

// Covers: the lines a prompt takes and what each comes to: approvals and
// plan approvals pick a numbered option, a plan's unknown mode left out;
// questions asked in turn and sent as one answer, by number or several
// numbers, the user's own text where allowed, numbers as text where they
// name no choice, a text field's line as typed, Enter taking a
// pre-selected default without repeats or skipping an optional question,
// /decline and /dismiss; a line that does not answer says why.
// Owner: rho attach prompt (unit).
#[test]
fn a_prompt_turns_typed_lines_into_its_answer() {
    struct Case {
        name: &'static str,
        ask: Ask,
        lines: &'static [&'static str],
        replies: Vec<Reply>,
    }
    let cases = [
        Case {
            name: "an approval picks a number",
            ask: approval(),
            lines: &["4", "", "2"],
            replies: vec![
                retry("answer a number from 1 to 3"),
                retry("answer a number from 1 to 3"),
                Reply::Answer(InputAnswer::AllowForSession),
            ],
        },
        Case {
            name: "a plan approval names a known mode, or keeps planning",
            ask: plan(),
            lines: &["0", "2", "3"],
            replies: vec![
                retry("answer a number from 1 to 3"),
                Reply::Answer(InputAnswer::ApprovePlan {
                    mode: PermissionMode::AllowEdits,
                    feedback: None,
                }),
                Reply::Answer(InputAnswer::KeepPlanning { feedback: None }),
            ],
        },
        Case {
            name: "questions by number, many numbers, and a text field",
            ask: questions(DefaultStyle::Selected),
            lines: &["1", "3, 2,3", "1"],
            replies: vec![
                Reply::Next,
                Reply::Next,
                answers(&[
                    ("db", &["pg"]),
                    ("features", &["bench", "ci"]),
                    ("name", &["1"]),
                ]),
            ],
        },
        Case {
            name: "what a pick of one choice refuses",
            ask: questions(DefaultStyle::Selected),
            lines: &["", "1 2", "3", ",", "postgres"],
            replies: vec![
                retry("this question needs an answer"),
                retry("pick one"),
                retry("there is no choice 3"),
                retry("pick by number"),
                retry("pick by number"),
            ],
        },
        Case {
            name: "Enter takes a pre-selected default; a text field needs its line",
            ask: questions(DefaultStyle::Selected),
            lines: &["2", "", "", "rho"],
            replies: vec![
                Reply::Next,
                Reply::Next,
                retry("this question needs an answer"),
                answers(&[
                    ("db", &["lite"]),
                    ("features", &["docs"]),
                    ("name", &["rho"]),
                ]),
            ],
        },
        Case {
            name: "a focused default is only suggested: Enter skips",
            ask: questions(DefaultStyle::Focused),
            lines: &["2", "", "rho"],
            replies: vec![
                Reply::Next,
                Reply::Next,
                answers(&[("db", &["lite"]), ("name", &["rho"])]),
            ],
        },
        Case {
            name: "own text where allowed, numbers naming no choice included",
            ask: questions(DefaultStyle::Selected),
            lines: &["1", "9", "rho"],
            replies: vec![
                Reply::Next,
                Reply::Next,
                answers(&[("db", &["pg"]), ("features", &["9"]), ("name", &["rho"])]),
            ],
        },
        Case {
            name: "/decline closes the questions",
            ask: questions(DefaultStyle::Selected),
            lines: &["1", "/decline"],
            replies: vec![Reply::Next, Reply::Answer(InputAnswer::Decline)],
        },
        Case {
            name: "/dismiss closes the questions",
            ask: questions(DefaultStyle::Selected),
            lines: &["/dismiss"],
            replies: vec![Reply::Answer(InputAnswer::Dismiss)],
        },
    ];
    for case in cases {
        let mut prompt = Prompt::new(&request(case.ask)).unwrap();
        let replies: Vec<Reply> = case.lines.iter().map(|line| prompt.reply(line)).collect();
        assert_eq!(replies, case.replies, "{}", case.name);
    }
}

// Covers: what a prompt shows: an approval's options; a plan approval's
// plan above its options, its unknown mode left out; each question with
// its help, choices and their descriptions, and how to answer it, a text
// field showing no placeholder choice; and no prompt for an empty question
// request.
// Owner: rho attach prompt (unit).
#[test]
fn a_prompt_shows_what_it_asks() {
    let mut asked = Prompt::new(&request(questions(DefaultStyle::Selected))).unwrap();
    let mut shown = vec![asked.opening()];
    for line in ["1", "2"] {
        assert_eq!(asked.reply(line), Reply::Next);
        shown.push(asked.asking());
    }
    let empty = Ask::Question(QuestionAsk {
        title: "Nothing".into(),
        questions: Vec::new(),
    });

    assert_eq!(
        (
            Prompt::new(&request(approval())).unwrap().opening(),
            Prompt::new(&request(plan())).unwrap().opening(),
            shown,
            Prompt::new(&request(empty)).is_none(),
        ),
        (
            "  1 allow once · 2 allow for session · 3 deny\n".to_owned(),
            "1. edit\n2. test\n  1 approve in auto · 2 approve in allow_edits · 3 keep planning\n"
                .to_owned(),
            vec![
                "  1/3 Which database?\n  pick what CI runs\n    1 postgres — the server\n    2 sqlite\n  pick one; /decline or /dismiss closes the questions\n".to_owned(),
                "  2/3 Features: Which features? (optional)\n    1 docs\n    2 CI\n    3 benchmarks\n  pick any, like 1,3; or type your own answer; Enter takes docs; /decline or /dismiss closes the questions\n".to_owned(),
                "  3/3 Project name?\n  type your answer; /decline or /dismiss closes the questions\n".to_owned(),
            ],
            true,
        )
    );
}
