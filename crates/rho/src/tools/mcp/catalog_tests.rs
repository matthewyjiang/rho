use pretty_assertions::assert_eq;

use super::{McpPrompt, McpPromptArgument};

fn prompt(arguments: Vec<McpPromptArgument>) -> McpPrompt {
    McpPrompt {
        server: "docs".into(),
        name: "search".into(),
        title: None,
        description: None,
        arguments,
    }
}

fn argument(name: &str, required: bool) -> McpPromptArgument {
    McpPromptArgument {
        name: name.into(),
        description: None,
        required,
    }
}

// Covers: typing a prompt's arguments must not force `key=value` syntax on the
// common single-argument case, and a required argument left out must be named
// rather than sent as an empty value the server will reject.
// Owner: pure unit
#[test]
fn prompt_arguments_parse_from_typed_text() {
    let single = prompt(vec![argument("query", true)]);
    let single_arguments = single.parse_arguments("  how do sessions resume  ");
    assert_eq!(
        single_arguments
            .get("query")
            .and_then(|value| value.as_str()),
        Some("how do sessions resume")
    );
    assert!(single.missing_arguments(&single_arguments).is_empty());

    let paired = prompt(vec![argument("query", true), argument("limit", false)]);
    let paired_arguments = paired.parse_arguments("query=sessions limit=5 stray");
    assert_eq!(
        (
            paired_arguments.get("query").and_then(|v| v.as_str()),
            paired_arguments.get("limit").and_then(|v| v.as_str()),
            paired_arguments.len(),
        ),
        (Some("sessions"), Some("5"), 2)
    );

    assert_eq!(
        paired.missing_arguments(&paired.parse_arguments("limit=5")),
        vec!["query"]
    );
    assert_eq!(
        paired.missing_arguments(&paired.parse_arguments("")),
        vec!["query"]
    );
}

// Covers: the palette must show a command name and usage line a user can act
// on, distinguishing required from optional arguments.
// Owner: pure unit
#[test]
fn prompt_command_name_and_usage_describe_the_call() {
    let prompt = prompt(vec![argument("query", true), argument("limit", false)]);

    assert_eq!(
        (prompt.command_name(), prompt.usage()),
        (
            "mcp:docs:search".to_string(),
            "/mcp:docs:search <query> [limit=…]".to_string()
        )
    );
}
