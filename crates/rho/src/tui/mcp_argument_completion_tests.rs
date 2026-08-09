use pretty_assertions::assert_eq;

use super::{
    argument_choices, argument_under_cursor, McpArgumentCompletions, McpCompletionKey,
    McpCompletionStep, PendingCompletion,
};
use crate::{
    tools::mcp::{
        catalog::{McpPrompt, McpPromptArgument},
        McpCompletionSupport,
    },
    tui::{tests::test_app, CommandChoice, CommandChoiceKind},
};

fn prompt(name: &str, arguments: &[&str]) -> McpPrompt {
    McpPrompt {
        server: "tickets".into(),
        name: name.into(),
        title: None,
        description: None,
        arguments: arguments
            .iter()
            .map(|argument| McpPromptArgument {
                name: (*argument).into(),
                description: None,
                required: false,
            })
            .collect(),
    }
}

fn detected(text: &str, cursor: usize, prompt: &McpPrompt) -> Option<(String, String)> {
    argument_under_cursor(text, cursor, prompt, McpCompletionSupport::Declared)
        .map(|cursor| (cursor.key.argument, cursor.key.typed))
}

fn key(argument: &str, typed: &str) -> McpCompletionKey {
    McpCompletionKey {
        server: "tickets".into(),
        prompt: "triage".into(),
        argument: argument.into(),
        typed: typed.into(),
    }
}

// Covers: suggestions offered for the wrong argument, or offered while the user
// is still typing an argument name or the command itself, would insert text
// where it does not belong.
// Owner: pure cursor detection for multi-argument prompts.
#[test]
fn cursor_position_picks_the_argument_value_being_typed() {
    let triage = prompt("triage", &["severity", "owner"]);
    let one_pair = "/mcp:tickets:triage severity=hi";
    let two_pairs = "/mcp:tickets:triage severity=hi owner=al";

    let cases = [
        // End of a value: that value is what the server is asked about.
        (one_pair, 31, Some(("severity", "hi"))),
        // Just past the `=`: the whole value is the query, not the prefix
        // before the cursor, so moving within a value asks nothing new.
        (one_pair, 29, Some(("severity", "hi"))),
        // Inside the argument name: a name is not a value.
        (one_pair, 23, None),
        // Inside the command token: command matching still owns the palette.
        (one_pair, 5, None),
        // On whitespace after a finished pair: no value has been started.
        ("/mcp:tickets:triage severity=hi ", 32, None),
        // The later pair wins when the cursor is in it.
        (two_pairs, 40, Some(("owner", "al"))),
        (two_pairs, 31, Some(("severity", "hi"))),
        // An argument the server never declared cannot be completed.
        ("/mcp:tickets:triage bogus=x", 27, None),
    ];

    for (text, cursor, expected) in cases {
        assert_eq!(
            detected(text, cursor, &triage),
            expected.map(|(argument, typed)| (argument.to_string(), typed.to_string())),
            "text {text:?} cursor {cursor}"
        );
    }
}

// Covers: a one-argument prompt takes free text rather than `key=value`, so
// requiring an `=` would leave the common case with no suggestions at all.
// Owner: pure cursor detection for single-argument prompts.
#[test]
fn single_argument_prompt_completes_the_whole_trailing_text() {
    let search = prompt("search", &["query"]);

    assert_eq!(
        detected("/mcp:tickets:search how do ses", 30, &search),
        Some(("query".to_string(), "how do ses".to_string()))
    );
    // Nothing typed yet still asks, because a server answers an empty value
    // with everything it can offer.
    assert_eq!(
        detected("/mcp:tickets:search ", 20, &search),
        Some(("query".to_string(), String::new()))
    );
    // Trailing whitespace is not part of the value the server is asked about.
    assert_eq!(
        detected("/mcp:tickets:search  bug  ", 26, &search),
        Some(("query".to_string(), "bug".to_string()))
    );
}

// Covers: asking a server that never declared `completions` can only earn an
// error, and one wasted round-trip per keystroke while someone types.
// Owner: capability gate on the completion request.
#[test]
fn server_without_the_completions_capability_is_never_asked() {
    let triage = prompt("triage", &["severity", "owner"]);
    let text = "/mcp:tickets:triage severity=hi";

    assert_eq!(
        argument_under_cursor(text, 31, &triage, McpCompletionSupport::Absent),
        None
    );
    assert!(argument_under_cursor(text, 31, &triage, McpCompletionSupport::Declared).is_some());
}

// Covers: a repeated query must not re-ask the server, and a failed query must
// not be retried on every pass of the event loop, which would spin one request
// per frame for as long as the value stays on screen.
// Owner: request policy over the suggestion cache.
#[test]
fn cached_and_failed_answers_both_stop_a_second_request() {
    let mut completions = McpArgumentCompletions::default();
    let wanted = key("severity", "hi");

    assert_eq!(
        completions.next_step(Some(&wanted)),
        McpCompletionStep::Ask(wanted.clone())
    );

    completions.store(wanted.clone(), vec!["high".into(), "highest".into()]);
    assert_eq!(
        completions.next_step(Some(&wanted)),
        McpCompletionStep::Wait
    );
    assert_eq!(
        completions.suggestions(&wanted),
        Some(["high".to_string(), "highest".to_string()].as_slice())
    );

    // A failure is stored as no suggestions, which is what the palette shows
    // and also what stops the next pass from asking again.
    let failed = key("owner", "al");
    completions.store(failed.clone(), Vec::new());
    assert_eq!(
        completions.next_step(Some(&failed)),
        McpCompletionStep::Wait
    );
    assert_eq!(completions.suggestions(&failed), Some([].as_slice()));

    assert_eq!(completions.next_step(None), McpCompletionStep::Wait);
}

// Covers: holding a key down must not pile up one task per character, so a
// request in flight has to suppress the next one until it lands.
// Owner: in-flight bound on completion requests.
#[tokio::test]
async fn a_request_in_flight_suppresses_the_next_one() {
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let mut completions = McpArgumentCompletions {
        pending: Some(PendingCompletion {
            key: key("severity", "h"),
            handle: tokio::spawn(async move {
                let _ = wait.await;
                Vec::new()
            }),
        }),
        ..Default::default()
    };

    assert!(completions.is_pending());
    assert_eq!(
        completions.next_step(Some(&key("severity", "high"))),
        McpCompletionStep::Wait
    );

    completions.cancel();
    assert!(!completions.is_pending());
    drop(release);
}

// Covers: a request that failed or has not landed must leave the palette with
// no argument rows at all, so nothing stale is shown and no error is raised
// while someone is still typing.
// Owner: palette rows built from the suggestion cache.
#[test]
fn a_failed_or_unanswered_lookup_offers_no_rows() {
    let triage = prompt("triage", &["severity", "owner"]);
    let cursor = argument_under_cursor(
        "/mcp:tickets:triage severity=hi",
        31,
        &triage,
        McpCompletionSupport::Declared,
    )
    .expect("cursor sits in the severity value");
    let mut unanswered = McpArgumentCompletions::default();
    assert!(argument_choices(&cursor, &unanswered).is_empty());

    // What the failure path records: the key, with nothing under it.
    unanswered.store(cursor.key.clone(), Vec::new());
    assert!(argument_choices(&cursor, &unanswered).is_empty());

    let mut answered = McpArgumentCompletions::default();
    answered.store(cursor.key.clone(), vec!["high".into()]);
    assert_eq!(
        argument_choices(&cursor, &answered)
            .into_iter()
            .map(|choice| (choice.name, choice.kind))
            .collect::<Vec<_>>(),
        vec![(
            "high".to_string(),
            CommandChoiceKind::McpPromptArgument { value: 29..31 }
        )]
    );
}

// Covers: a picked suggestion that rewrote the whole command would discard the
// command name and every other argument already typed.
// Owner: composer edit applied when a suggestion is chosen.
//
// Not a PTY scenario: driving this end to end needs a live MCP server that
// declares `completions` and answers it, and the harness has no such server.
#[test]
fn picking_a_suggestion_replaces_only_that_argument_value() {
    let mut app = test_app();
    let text = "/mcp:tickets:triage severity=hi owner=al";
    app.input_ui.set_text(text.to_string());
    app.input_ui.set_cursor(text.chars().count());

    app.complete_command_choice(&CommandChoice {
        name: "alice".into(),
        usage: "alice".into(),
        description: "owner · suggested by MCP server `tickets`".into(),
        kind: CommandChoiceKind::McpPromptArgument { value: 38..40 },
    });

    assert_eq!(
        (app.input_ui.text(), app.input_ui.cursor()),
        ("/mcp:tickets:triage severity=hi owner=alice", 43)
    );
}
