use pretty_assertions::assert_eq;

use super::{parse_answers, AnswerStyle};
use crate::decision::{ChoiceOption, ChoiceQuestion};

const COLOR: ChoiceQuestion = ChoiceQuestion {
    id: "color",
    instructions: "Pick a color.",
    options: &[
        ChoiceOption {
            id: "red",
            description: "red",
        },
        ChoiceOption {
            id: "blue",
            description: "blue",
        },
    ],
};

const SIZE: ChoiceQuestion = ChoiceQuestion {
    id: "size",
    instructions: "Pick a size.",
    options: &[
        ChoiceOption {
            id: "small",
            description: "small",
        },
        ChoiceOption {
            id: "large",
            description: "large",
        },
    ],
};

// Covers: the answer is exactly the whole direct response or the final line
// of a reasoned one, never text before it. A reasoned response with a code
// fence or a line break other than LF or CRLF anywhere, which could make its
// last line quoted text, is not read. The answer must name exactly the questions asked, each
// once (escaped or not), with an option ID.
// Owner: decision protocol LLM adapter.
#[test]
fn answers_are_exactly_the_final_answer_line() {
    use AnswerStyle::{Direct, Reasoned};
    let questions = [COLOR, SIZE];
    let cases = [
        (Direct, r#" {"color":"red","size":"large"} "#, Some(["red", "large"])),
        (Direct, "```json\n{\"color\":\"red\",\"size\":\"large\"}\n```", None),
        (Direct, r#"Sure: {"color":"red","size":"large"}"#, None),
        (
            Direct,
            "{\"color\":\"blue\",\"size\":\"small\"}\n{\"color\":\"red\",\"size\":\"large\"}",
            None,
        ),
        (
            Reasoned,
            "Weighing {color: \"}\" first.\r\n{\"color\":\"blue\",\"size\":\"small\"}\r\n",
            Some(["blue", "small"]),
        ),
        (
            Reasoned,
            "```text\n{\"color\":\"red\",\"size\":\"large\"}\n```\n{\"color\":\"blue\",\"size\":\"small\"}",
            None,
        ),
        (
            Reasoned,
            "````text\n```\n{\"color\":\"red\",\"size\":\"large\"}",
            None,
        ),
        (
            Reasoned,
            "~~~json\n{\"color\":\"red\",\"size\":\"large\"}",
            None,
        ),
        (
            Reasoned,
            "Reasoning.\n{\"color\":\"red\",\"size\":\"large\"}\n \t\n",
            Some(["red", "large"]),
        ),
        (
            Reasoned,
            "Run `a ``` b` first.\n{\"color\":\"red\",\"size\":\"large\"}",
            None,
        ),
        (
            Reasoned,
            r#"{"color":"red","size":"large","\u0063olor":"blue"}"#,
            None,
        ),
        (
            Reasoned,
            "Quoted payload:\n```json\n{\"color\":\"red\",\"size\":\"large\"}",
            None,
        ),
        (
            Reasoned,
            "Reasoning.\n```json\n{\"color\":\"blue\",\"size\":\"small\"}\n```",
            None,
        ),
        (
            Reasoned,
            r#"The answer is {"color":"red","size":"small"}"#,
            None,
        ),
        (
            Reasoned,
            r#"{"color":"blue","size":"large","why":{"color":"red","size":"small"}}"#,
            None,
        ),
        (
            Reasoned,
            r#"{"color":"blue","color":"red","size":"large"}"#,
            None,
        ),
        (
            Reasoned,
            r#"{"color":"red","size":"large","color":"blue"}"#,
            None,
        ),
        (
            Reasoned,
            r#"{"color":"red","size":"large","shape":"round"}"#,
            None,
        ),
        (Reasoned, r#"{"color":"Red","size":"large"}"#, None),
        (Reasoned, r#"{"color":"green","size":"large"}"#, None),
        (Reasoned, r#"{"color":"red"}"#, None),
        (Reasoned, r#"{"color":["red"],"size":"large"}"#, None),
    ];

    let answer = r#"{"color":"red","size":"large"}"#;
    let separated = ["\r", "\u{0B}", "\u{0C}", "\u{85}", "\u{2028}", "\u{2029}"]
        .into_iter()
        .flat_map(|separator| {
            [
                format!("Reasoning.{separator}x\n{answer}"),
                format!("{separator}Reasoning.\n{answer}"),
                format!("Reasoning.\n{answer}{separator}"),
            ]
        });
    for text in separated {
        assert!(
            parse_answers(&text, &questions, Reasoned).is_err(),
            "{text:?}"
        );
    }

    for (style, text, expected) in cases {
        let actual = parse_answers(text, &questions, style)
            .ok()
            .map(|answers| [answers["color"].option.id, answers["size"].option.id]);
        assert_eq!(actual, expected, "{style:?} {text}");
    }
}

// Covers: a parse error never repeats response text, which may hold secrets
// from the state.
// Owner: decision protocol LLM adapter.
#[test]
fn parse_errors_do_not_repeat_the_response() {
    const SECRET: &str = "sk-echoed";
    let questions = [COLOR, SIZE];
    let responses = [
        format!("\"{SECRET}\""),
        format!(r#"{{"{SECRET}":"red","{SECRET}":"red"}}"#),
        format!(r#"{{"color":"{SECRET}","size":"large"}}"#),
        format!(r#"{{"color":"red","{SECRET}":"large"}}"#),
    ];
    for response in responses {
        let error = parse_answers(&response, &questions, AnswerStyle::Direct).unwrap_err();
        let error = format!("{error:#}");
        assert!(!error.contains(SECRET), "{error}");
    }
}

// Covers: a question no System One server would accept is caught when the
// question is declared.
// Owner: decision protocol question checks.
#[test]
fn invalid_questions_fail_validation() {
    const fn option(id: &'static str) -> ChoiceOption {
        ChoiceOption {
            id,
            description: "",
        }
    }
    const A: ChoiceOption = option("a");
    const EMPTY: ChoiceOption = option("");
    const TOO_MANY: [ChoiceOption; 27] = [const { option("x") }; 27];
    let cases: [(&str, ChoiceQuestion); 5] = [
        (
            "one option",
            ChoiceQuestion {
                options: &[A],
                ..COLOR
            },
        ),
        (
            "27 options",
            ChoiceQuestion {
                options: &TOO_MANY,
                ..COLOR
            },
        ),
        (
            "duplicate option",
            ChoiceQuestion {
                options: &[A, A],
                ..COLOR
            },
        ),
        (
            "space in question ID",
            ChoiceQuestion {
                id: "pick color",
                ..COLOR
            },
        ),
        (
            "empty option ID",
            ChoiceQuestion {
                options: &[A, EMPTY],
                ..COLOR
            },
        ),
    ];

    COLOR.validate();
    for (name, question) in cases {
        let result = std::panic::catch_unwind(|| question.validate());
        assert!(result.is_err(), "{name} passed validation");
    }
}
