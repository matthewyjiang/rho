use pretty_assertions::assert_eq;

use super::*;
use crate::model::ModelEvent;
use rho_sdk::model::ASYNC_TOOL_CALL_CONTEXT_KIND;

fn async_markers(events: &[ModelEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            ModelEvent::ProviderContext {
                kind,
                position,
                data,
            } if kind == ASYNC_TOOL_CALL_CONTEXT_KIND && position.is_none() => {
                data.as_str().map(str::to_owned)
            }
            _ => None,
        })
        .collect()
}

// Covers: async function_call items emit the SDK marker once even when
// response.completed restates the same output item.
// Owner: openai Responses SSE parse
#[test]
fn async_function_call_emits_marker_once() {
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
            name: "sync function_call never marks",
            lines: vec![
                r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"bash","arguments":"{}"}}"#,
                r#"data: {"type":"response.completed","response":{"id":"resp_1","output":[{"type":"function_call","call_id":"call_1","name":"bash","arguments":"{}"}]}}"#,
            ],
            expected: vec![],
        },
        Case {
            name: "completed-only async item still marks",
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
        assert_eq!(async_markers(&events), case.expected, "{}", case.name);
    }
}
