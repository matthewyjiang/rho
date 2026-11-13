use pretty_assertions::assert_eq;

use super::*;
use crate::model::ModelEvent;
use rho_sdk::tool::ToolExecutionMode;

fn async_acceptances(events: &[ModelEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ToolCallAccepted {
                id,
                execution: ToolExecutionMode::Async,
            } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

// Covers: async function_call items emit one async ToolCallAccepted even when
// response.completed restates the same output item.
// Owner: openai Responses SSE parse
#[test]
fn async_function_call_is_accepted_once() {
    struct Case {
        name: &'static str,
        lines: Vec<&'static str>,
        expected: Vec<&'static str>,
    }
    let cases = [
        Case {
            name: "async true on done and completed",
            lines: vec![
                r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"one_agent","arguments":"{}","async":true}}"#,
                r#"data: {"type":"response.completed","response":{"id":"resp_1","output":[{"type":"function_call","call_id":"call_1","name":"one_agent","arguments":"{}","async":true}]}}"#,
            ],
            expected: vec!["call_1"],
        },
        Case {
            name: "sync function_call is never accepted as async",
            lines: vec![
                r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"bash","arguments":"{}"}}"#,
                r#"data: {"type":"response.completed","response":{"id":"resp_1","output":[{"type":"function_call","call_id":"call_1","name":"bash","arguments":"{}"}]}}"#,
            ],
            expected: vec![],
        },
        Case {
            name: "completed-only async item is still accepted",
            lines: vec![
                r#"data: {"type":"response.completed","response":{"id":"resp_1","output":[{"type":"function_call","call_id":"call_9","name":"one_agent","arguments":"{}","async":true}]}}"#,
            ],
            expected: vec!["call_9"],
        },
    ];

    for case in cases {
        let mut state = CodexSseState::default();
        let mut events = Vec::new();
        for line in case.lines {
            handle_codex_sse_line(
                line,
                &mut state,
                &mut Some(&mut |event| {
                    events.push(event);
                    Ok(())
                }),
            )
            .unwrap();
        }
        assert_eq!(async_acceptances(&events), case.expected, "{}", case.name);
    }
}
