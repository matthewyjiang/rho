use pretty_assertions::assert_eq;

use crate::{subagent::RunState, tui::AttachmentEvent};

use super::*;

fn effects_from_fixture(name: &str) -> Vec<StreamEffect> {
    let body = match name {
        "success.ndjson" => include_str!("../fixtures/success.ndjson"),
        "live_success.ndjson" => include_str!("../fixtures/live_success.ndjson"),
        "tool_call.ndjson" => include_str!("../fixtures/tool_call.ndjson"),
        "error_result.ndjson" => include_str!("../fixtures/error_result.ndjson"),
        "unknown_and_partial.ndjson" => include_str!("../fixtures/unknown_and_partial.ndjson"),
        "mixed_partial_complete.ndjson" => {
            include_str!("../fixtures/mixed_partial_complete.ndjson")
        }
        "partial_tool_complete_text.ndjson" => {
            include_str!("../fixtures/partial_tool_complete_text.ndjson")
        }
        "message_start_no_deltas.ndjson" => {
            include_str!("../fixtures/message_start_no_deltas.ndjson")
        }
        "missing_message_id.ndjson" => include_str!("../fixtures/missing_message_id.ndjson"),
        "live_tool_roundtrip.ndjson" => include_str!("../fixtures/live_tool_roundtrip.ndjson"),
        other => panic!("unknown fixture {other}"),
    };
    let mut mapper = StreamMapper::new();
    body.lines()
        .flat_map(|line| mapper.push_line(line))
        .collect()
}

fn count_attachments(
    effects: &[StreamEffect],
    predicate: impl Fn(&AttachmentEvent) -> bool,
) -> usize {
    effects
        .iter()
        .filter(|effect| match effect {
            StreamEffect::Attachment(event) => predicate(event),
            _ => false,
        })
        .count()
}

fn joined_text(effects: &[StreamEffect]) -> String {
    effects
        .iter()
        .filter_map(|effect| match effect {
            StreamEffect::Attachment(AttachmentEvent::AssistantTextDelta(text)) => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect()
}

fn joined_reasoning(effects: &[StreamEffect]) -> String {
    effects
        .iter()
        .filter_map(|effect| match effect {
            StreamEffect::Attachment(AttachmentEvent::ReasoningDelta(text)) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn assert_no_terminal_attachment(effects: &[StreamEffect]) {
    assert_eq!(
        count_attachments(effects, |event| {
            matches!(
                event,
                AttachmentEvent::Completed | AttachmentEvent::Failed(_)
            )
        }),
        0,
        "result mapping must not emit Completed/Failed"
    );
}

#[test]
fn maps_success_result_with_plan_high_cache_context_usage() {
    let effects = effects_from_fixture("success.ndjson");
    let terminal = effects.iter().find_map(|effect| match effect {
        StreamEffect::Terminal(terminal) => Some(terminal),
        _ => None,
    });
    let terminal = terminal.expect("terminal result");
    assert!(terminal.classification.is_success());
    assert_eq!(terminal.session_id.as_deref(), Some("sess-success-001"));
    assert_eq!(terminal.num_turns, Some(1));
    assert_eq!(terminal.total_cost_usd, Some(0.0388));
    let usage = terminal.usage.as_ref().expect("usage");
    // PLAN high-cache example: uncached + cache-read + cache-creation.
    assert_eq!(usage.input_tokens, Some(2));
    assert_eq!(usage.cache_read_tokens, Some(7294));
    assert_eq!(usage.cache_write_tokens, Some(6095));
    assert_eq!(usage.total_input_tokens(), Some(2 + 7294 + 6095));
    assert_eq!(usage.output_tokens, Some(4));

    let context = terminal.context.as_ref().expect("context");
    assert_eq!(context.tokens, Some(2 + 7294 + 6095));
    assert_eq!(context.context_window, Some(200_000));
    assert_eq!(
        context.source,
        rho_sdk::model::ContextUsageSource::ProviderReported
    );

    let mut status = crate::subagent::RunStatus::default();
    for effect in &effects {
        if let StreamEffect::Status(patch) = effect {
            apply_status_patch(&mut status, patch.clone());
        }
    }
    // Result messages no longer publish terminal Ok/Error on the status patch.
    assert_ne!(status.state, RunState::Ok);
    assert_ne!(status.state, RunState::Error);
    assert_eq!(status.input_tokens, 2 + 7294 + 6095);
    assert_eq!(status.output_tokens, 4);
    assert_eq!(
        status.claude_session_id.as_deref(),
        Some("sess-success-001")
    );
    assert_eq!(status.total_cost_usd, Some(0.0388));
    assert_eq!(status.result.as_deref(), Some("Hello from Claude."));
    assert_no_terminal_attachment(&effects);
    assert!(effects
        .iter()
        .any(|effect| matches!(effect, StreamEffect::RateLimit(_))));
    assert!(effects.iter().any(|effect| {
        matches!(
            effect,
            StreamEffect::Attachment(AttachmentEvent::ContextUsage(usage))
                if usage.tokens == Some(2 + 7294 + 6095)
                    && usage.context_window == Some(200_000)
        )
    }));
}

#[test]
fn maps_live_success_capture_round_trip() {
    let effects = effects_from_fixture("live_success.ndjson");
    let terminal = effects.iter().find_map(|effect| match effect {
        StreamEffect::Terminal(terminal) => Some(terminal),
        _ => None,
    });
    let terminal = terminal.expect("terminal result");
    assert!(terminal.classification.is_success());
    assert_eq!(
        terminal.session_id.as_deref(),
        Some("11111111-2222-4333-8444-555555555555")
    );
    assert_eq!(terminal.num_turns, Some(1));
    assert_eq!(terminal.result_text.as_deref(), Some("rho-claude-e2e-ok"));
    let usage = terminal.usage.as_ref().expect("usage");
    assert_eq!(usage.input_tokens, Some(2));
    assert_eq!(usage.cache_read_tokens, Some(3289));
    assert_eq!(usage.cache_write_tokens, Some(5413));
    assert_eq!(usage.total_input_tokens(), Some(2 + 3289 + 5413));
    assert_eq!(usage.output_tokens, Some(14));

    let context = terminal.context.as_ref().expect("context");
    assert_eq!(context.tokens, Some(2 + 3289 + 5413));
    // Max contextWindow across modelUsage entries (haiku 200k, sonnet 1m).
    assert_eq!(context.context_window, Some(1_000_000));
    assert_eq!(
        context.source,
        rho_sdk::model::ContextUsageSource::ProviderReported
    );

    // Partials streamed before the complete assistant envelope.
    assert_eq!(joined_text(&effects), "rho-claude-e2e-ok");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::StepStarted)
        }),
        1
    );
    assert!(effects
        .iter()
        .any(|effect| matches!(effect, StreamEffect::RateLimit(_))));
    assert!(effects.iter().any(|effect| {
        matches!(
            effect,
            StreamEffect::Attachment(AttachmentEvent::Usage(usage))
                if usage.output_tokens == Some(14)
        )
    }));
    assert!(effects.iter().any(|effect| {
        matches!(
            effect,
            StreamEffect::Attachment(AttachmentEvent::ContextUsage(usage))
                if usage.tokens == Some(2 + 3289 + 5413)
                    && usage.context_window == Some(1_000_000)
        )
    }));

    let mut status = crate::subagent::RunStatus::default();
    for effect in &effects {
        if let StreamEffect::Status(patch) = effect {
            apply_status_patch(&mut status, patch.clone());
        }
    }
    assert_ne!(status.state, RunState::Ok);
    assert_ne!(status.state, RunState::Error);
    assert_eq!(status.input_tokens, 2 + 3289 + 5413);
    assert_eq!(status.output_tokens, 14);
    assert_eq!(
        status.claude_session_id.as_deref(),
        Some("11111111-2222-4333-8444-555555555555")
    );
    assert_eq!(status.result.as_deref(), Some("rho-claude-e2e-ok"));
    assert_no_terminal_attachment(&effects);
}

#[test]
fn maps_tool_call_and_result_display() {
    let effects = effects_from_fixture("tool_call.ndjson");
    let started = effects.iter().find_map(|effect| match effect {
        StreamEffect::Attachment(AttachmentEvent::ToolStarted { display_lines }) => {
            Some(display_lines.clone())
        }
        _ => None,
    });
    let started = started.expect("tool started");
    assert!(started[0].contains("Read"));
    let finished = effects.iter().any(|effect| {
        matches!(
            effect,
            StreamEffect::Attachment(AttachmentEvent::ToolFinished { ok: true, .. })
        )
    });
    assert!(finished);
    assert!(effects.iter().any(|effect| {
        matches!(
            effect,
            StreamEffect::Attachment(AttachmentEvent::AssistantTextDelta(text)) if text.contains("Done.")
        )
    }));
    assert_no_terminal_attachment(&effects);
}

#[test]
fn maps_error_result_and_permission_denials() {
    let effects = effects_from_fixture("error_result.ndjson");
    let terminal = effects.iter().find_map(|effect| match effect {
        StreamEffect::Terminal(terminal) => Some(terminal),
        _ => None,
    });
    let terminal = terminal.expect("terminal");
    assert!(terminal.classification.is_failure());
    assert!(!terminal.permission_denials.is_empty());
    assert!(effects.iter().any(|effect| matches!(
        effect,
        StreamEffect::Attachment(AttachmentEvent::Notice(text))
            if text.contains("permission denied")
    )));
    assert_no_terminal_attachment(&effects);
    let mut status = crate::subagent::RunStatus::default();
    for effect in &effects {
        if let StreamEffect::Status(patch) = effect {
            apply_status_patch(&mut status, patch.clone());
        }
    }
    // Failure text is carried on the patch/terminal; session sets RunState.
    assert_eq!(status.error.as_deref(), Some("hit max turns"));
    assert_ne!(status.state, RunState::Ok);
}

#[test]
fn unknown_messages_do_not_fail_and_partials_stream() {
    let effects = effects_from_fixture("unknown_and_partial.ndjson");
    assert!(effects.iter().any(|effect| matches!(
        effect,
        StreamEffect::Attachment(AttachmentEvent::Notice(text))
            if text.contains("unknown message type")
    )));
    assert_eq!(joined_text(&effects), "chunk-two");
    assert!(effects.iter().any(|effect| matches!(
        effect,
        StreamEffect::Terminal(terminal) if terminal.classification.is_success()
    )));
    assert_no_terminal_attachment(&effects);
}

#[test]
fn mixed_partial_and_complete_envelopes_emit_presentation_once() {
    let effects = effects_from_fixture("mixed_partial_complete.ndjson");

    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::StepStarted)
        }),
        1,
        "StepStarted once"
    );
    assert_eq!(joined_text(&effects), "Reading now.");
    assert_eq!(joined_reasoning(&effects), "plan step");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolStarted { .. })
        }),
        1,
        "ToolStarted once"
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolFinished { ok: true, .. })
        }),
        1,
        "ToolFinished once"
    );
    assert_no_terminal_attachment(&effects);

    let terminal = effects
        .iter()
        .find_map(|effect| match effect {
            StreamEffect::Terminal(terminal) => Some(terminal),
            _ => None,
        })
        .expect("terminal");
    assert!(terminal.classification.is_success());
    assert_eq!(
        terminal
            .usage
            .as_ref()
            .and_then(ModelUsage::total_input_tokens),
        Some(3 + 10 + 7)
    );
    assert_eq!(
        terminal.context.as_ref().and_then(|context| context.tokens),
        Some(3 + 10 + 7)
    );
}

#[test]
fn partial_tool_only_plus_complete_only_text_and_reasoning() {
    // Tool streamed via deltas; text and reasoning arrive only on the complete
    // assistant envelope. Each presentation event must fire exactly once.
    let effects = effects_from_fixture("partial_tool_complete_text.ndjson");

    assert_eq!(
        count_attachments(&effects, |event| matches!(
            event,
            AttachmentEvent::StepStarted
        )),
        1
    );
    assert_eq!(joined_reasoning(&effects), "think complete-only");
    assert_eq!(joined_text(&effects), "text complete-only");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolStarted { display_lines }
                if display_lines.iter().any(|line| line.contains("toolu_partial_1")))
        }),
        1
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolFinished { ok: true, .. })
        }),
        1
    );
    assert_no_terminal_attachment(&effects);
}

#[test]
fn message_start_without_deltas_emits_complete_blocks_once() {
    let effects = effects_from_fixture("message_start_no_deltas.ndjson");
    assert_eq!(
        count_attachments(&effects, |event| matches!(
            event,
            AttachmentEvent::StepStarted
        )),
        1
    );
    assert_eq!(joined_text(&effects), "only on complete");
    assert_eq!(joined_reasoning(&effects), "reason complete");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolStarted { .. })
        }),
        1
    );
    assert_no_terminal_attachment(&effects);
}

#[test]
fn missing_message_ids_do_not_share_fallback_or_duplicate() {
    let effects = effects_from_fixture("missing_message_id.ndjson");
    // Two distinct complete assistants without ids must each emit StepStarted
    // and their own text once. No shared "unknown" key merges them.
    assert_eq!(
        count_attachments(&effects, |event| matches!(
            event,
            AttachmentEvent::StepStarted
        )),
        2
    );
    assert_eq!(joined_text(&effects), "first second");
    assert_no_terminal_attachment(&effects);
}

#[test]
fn some_blocks_streamed_others_complete_only_within_one_message() {
    // index 0 thinking streamed; index 1 text complete-only; index 2 tool streamed.
    let lines = [
        r#"{"type":"stream_event","session_id":"s","event":{"type":"message_start","message":{"id":"msg_mix","role":"assistant"}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"streamed think"}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_stop","index":0}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_x","name":"Read","input":{}}}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"content_block_stop","index":2}}"#,
        r#"{"type":"stream_event","session_id":"s","event":{"type":"message_stop"}}"#,
        r#"{"type":"assistant","session_id":"s","message":{"id":"msg_mix","role":"assistant","content":[{"type":"thinking","thinking":"streamed think"},{"type":"text","text":"complete-only text"},{"type":"tool_use","id":"toolu_x","name":"Read","input":{"path":"a"}}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_x","content":"ok"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"s","usage":{"input_tokens":1,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}"#,
    ];
    let mut mapper = StreamMapper::new();
    let effects: Vec<_> = lines
        .iter()
        .flat_map(|line| mapper.push_line(line))
        .collect();

    assert_eq!(
        count_attachments(&effects, |event| matches!(
            event,
            AttachmentEvent::StepStarted
        )),
        1
    );
    assert_eq!(joined_reasoning(&effects), "streamed think");
    assert_eq!(joined_text(&effects), "complete-only text");
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolStarted { .. })
        }),
        1
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolFinished { ok: true, .. })
        }),
        1
    );
    assert_no_terminal_attachment(&effects);
}

#[test]
fn terminal_subtype_is_error_matrix_never_defaults_to_success() {
    let cases = [
        (
            Some("success"),
            Some(false),
            TerminalClassification::Success {
                subtype: "success".into(),
            },
        ),
        (
            Some("success"),
            Some(true),
            TerminalClassification::Failure {
                subtype: "success".into(),
                is_error: true,
            },
        ),
        (
            Some("error_max_turns"),
            Some(true),
            TerminalClassification::Failure {
                subtype: "error_max_turns".into(),
                is_error: true,
            },
        ),
        (
            Some("error_max_turns"),
            Some(false),
            TerminalClassification::Failure {
                subtype: "error_max_turns".into(),
                is_error: false,
            },
        ),
        (
            Some("error_during_execution"),
            Some(true),
            TerminalClassification::Failure {
                subtype: "error_during_execution".into(),
                is_error: true,
            },
        ),
        (
            None,
            Some(false),
            TerminalClassification::Invalid {
                reason: "claude result missing subtype (is_error=false)".into(),
            },
        ),
        (
            None,
            Some(true),
            TerminalClassification::Invalid {
                reason: "claude result missing subtype (is_error=true)".into(),
            },
        ),
        (
            Some("success"),
            None,
            TerminalClassification::Invalid {
                reason: "claude result subtype `success` missing is_error".into(),
            },
        ),
        (
            None,
            None,
            TerminalClassification::Invalid {
                reason: "claude result missing subtype and is_error".into(),
            },
        ),
        (
            Some(""),
            Some(false),
            TerminalClassification::Invalid {
                reason: "claude result missing subtype (is_error=false)".into(),
            },
        ),
    ];

    for (subtype, is_error, expected) in cases {
        assert_eq!(
            classify_terminal_result(subtype, is_error),
            expected,
            "subtype={subtype:?} is_error={is_error:?}"
        );
    }

    // Parsed result messages with schema drift must not look like success and
    // must not publish RunState::Ok or terminal attachments on the status patch.
    let missing = map_line(r#"{"type":"result","result":"oops"}"#);
    let terminal = missing.iter().find_map(|effect| match effect {
        StreamEffect::Terminal(terminal) => Some(terminal),
        _ => None,
    });
    let terminal = terminal.expect("terminal");
    assert!(terminal.classification.is_invalid());
    assert!(!terminal.classification.is_success());
    assert_no_terminal_attachment(&missing);
    assert!(!missing.iter().any(|effect| {
        matches!(
            effect,
            StreamEffect::Status(StatusPatch {
                state: Some(RunState::Ok),
                ..
            })
        )
    }));
    assert!(!missing.iter().any(|effect| {
        matches!(
            effect,
            StreamEffect::Status(StatusPatch {
                state: Some(RunState::Error),
                ..
            })
        )
    }));
}

#[test]
fn stream_error_messages_are_pending_metadata_not_terminal() {
    let effects = map_line(r#"{"type":"error","result":"boom"}"#);
    assert_no_terminal_attachment(&effects);
    assert!(!effects.iter().any(|effect| {
        matches!(
            effect,
            StreamEffect::Status(StatusPatch {
                state: Some(RunState::Error | RunState::Ok | RunState::Stopped),
                ..
            })
        )
    }));
    assert!(effects.iter().any(|effect| {
        matches!(
            effect,
            StreamEffect::Status(StatusPatch {
                error: Some(text),
                last_activity: Some(activity),
                ..
            }) if text == "boom" && activity == "error received"
        )
    }));
    let terminal = effects.iter().find_map(|effect| match effect {
        StreamEffect::Terminal(terminal) => Some(terminal),
        _ => None,
    });
    let terminal = terminal.expect("pending failure terminal");
    assert!(terminal.classification.is_failure());
    assert_eq!(terminal.error.as_deref(), Some("boom"));
}

#[test]
fn aggregates_multiple_model_usage_entries_deterministically() {
    let line = r#"{
        "type":"result",
        "subtype":"success",
        "is_error":false,
        "usage":{"input_tokens":1,"output_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":4},
        "modelUsage":{
            "z-model":{"contextWindow":100000,"inputTokens":9},
            "a-model":{"contextWindow":200000,"inputTokens":1,"cacheReadInputTokens":3,"cacheCreationInputTokens":4}
        }
    }"#;
    let effects = map_line(line);
    let terminal = effects
        .iter()
        .find_map(|effect| match effect {
            StreamEffect::Terminal(terminal) => Some(terminal),
            _ => None,
        })
        .expect("terminal");
    // Prefer top-level usage total so RunStatus and ContextUsage match.
    assert_eq!(
        terminal
            .usage
            .as_ref()
            .and_then(ModelUsage::total_input_tokens),
        Some(1 + 3 + 4)
    );
    assert_eq!(
        terminal.context.as_ref().and_then(|context| context.tokens),
        Some(1 + 3 + 4)
    );
    // Context window is the max across modelUsage entries (sorted by key).
    assert_eq!(
        terminal
            .context
            .as_ref()
            .and_then(|context| context.context_window),
        Some(200_000)
    );
    assert_no_terminal_attachment(&effects);
}

#[test]
fn bounds_retained_text_result_and_tool_payloads() {
    let huge = "x".repeat(MAX_TEXT_DELTA_CHARS + 50);
    let text_line = format!(
        r#"{{"type":"assistant","message":{{"id":"m1","content":[{{"type":"text","text":"{huge}"}}]}}}}"#
    );
    let effects = map_line(&text_line);
    let delta = effects.iter().find_map(|effect| match effect {
        StreamEffect::Attachment(AttachmentEvent::AssistantTextDelta(text)) => Some(text.as_str()),
        _ => None,
    });
    let delta = delta.expect("text delta");
    assert!(delta.contains("truncated text"));
    assert!(delta.chars().count() < huge.chars().count());

    let huge_result = "r".repeat(MAX_RESULT_CHARS + 20);
    let result_line = format!(
        r#"{{"type":"result","subtype":"success","is_error":false,"result":"{huge_result}"}}"#
    );
    let effects = map_line(&result_line);
    let terminal = effects
        .iter()
        .find_map(|effect| match effect {
            StreamEffect::Terminal(terminal) => Some(terminal),
            _ => None,
        })
        .expect("terminal");
    let result = terminal.result_text.as_deref().expect("result text");
    assert!(result.contains("truncated result"));
    assert_no_terminal_attachment(&effects);

    let huge_tool = "t".repeat(MAX_TOOL_PAYLOAD_CHARS + 30);
    let tool_line = format!(
        r#"{{"type":"assistant","message":{{"id":"m2","content":[{{"type":"tool_use","id":"toolu_big","name":"Bash","input":{{"cmd":"{huge_tool}"}}}}]}}}}"#
    );
    let effects = map_line(&tool_line);
    let started = effects.iter().find_map(|effect| match effect {
        StreamEffect::Attachment(AttachmentEvent::ToolStarted { display_lines }) => {
            Some(display_lines.join("\n"))
        }
        _ => None,
    });
    let started = started.expect("tool started");
    assert!(started.contains("truncated") || started.len() < huge_tool.len() + 64);
}

#[test]
fn malformed_json_becomes_notice() {
    let effects = map_line("{not-json");
    assert!(matches!(
        effects.as_slice(),
        [StreamEffect::Attachment(AttachmentEvent::Notice(text))]
            if text.contains("malformed JSON")
    ));
}

#[test]
fn pending_result_status_uses_result_received_activity() {
    let effects = map_line(
        r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"s1"}"#,
    );
    assert!(effects.iter().any(|effect| matches!(
        effect,
        StreamEffect::Status(StatusPatch {
            last_activity: Some(activity),
            ..
        }) if activity == "result received"
    )));
    assert_no_terminal_attachment(&effects);
}

#[test]
fn protocol_control_frames_are_silent_noops() {
    // Documented progress/control frames must not emit diagnostics or status.
    for line in [
        r#"{"type":"tool_progress","tool_use_id":"toolu_1","elapsed_time_seconds":1}"#,
        r#"{"type":"status","status":"requesting"}"#,
        r#"{"type":"keep_alive"}"#,
        r#"{"type":"control_request","request_id":"1"}"#,
        r#"{"type":"control_response","request_id":"1"}"#,
    ] {
        let effects = map_line(line);
        assert!(
            effects.is_empty(),
            "expected silent no-op for {line}, got {effects:?}"
        );
    }
}

#[test]
fn unknown_top_level_kind_emits_diagnostic_notice() {
    let effects = map_line(r#"{"type":"brand_new_frame","payload":true}"#);
    assert!(matches!(
        effects.as_slice(),
        [StreamEffect::Attachment(AttachmentEvent::Notice(text))]
            if text.contains("unknown message type") && text.contains("brand_new_frame")
    ));
}

#[test]
fn system_status_heartbeat_is_quiet_but_init_is_noticed() {
    let status = map_line(
        r#"{"type":"system","subtype":"status","status":"requesting","session_id":"sess-hb"}"#,
    );
    assert!(
        !status
            .iter()
            .any(|effect| matches!(effect, StreamEffect::Attachment(AttachmentEvent::Notice(_)))),
        "status heartbeat must not spam notices: {status:?}"
    );
    assert!(status.iter().any(|effect| matches!(
        effect,
        StreamEffect::Status(StatusPatch {
            claude_session_id: Some(id),
            ..
        }) if id == "sess-hb"
    )));

    let init = map_line(r#"{"type":"system","subtype":"init","session_id":"sess-init"}"#);
    assert!(init.iter().any(|effect| matches!(
        effect,
        StreamEffect::Attachment(AttachmentEvent::Notice(text)) if text.contains("claude system: init")
    )));
}

#[test]
fn maps_live_tool_roundtrip_capture() {
    let effects = effects_from_fixture("live_tool_roundtrip.ndjson");

    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolStarted { display_lines }
                if display_lines.iter().any(|line| line.contains("Read"))
                    && display_lines.iter().any(|line| line.contains("toolu_0liveToolRoundtrip")))
        }),
        1,
        "tool start"
    );
    assert_eq!(
        count_attachments(&effects, |event| {
            matches!(event, AttachmentEvent::ToolFinished { ok: true, .. })
        }),
        1,
        "tool finish"
    );
    assert!(
        joined_text(&effects).contains("rho-tool-fixture-marker-42"),
        "assistant final text: {}",
        joined_text(&effects)
    );

    let terminal = effects
        .iter()
        .find_map(|effect| match effect {
            StreamEffect::Terminal(terminal) => Some(terminal),
            _ => None,
        })
        .expect("terminal");
    assert!(terminal.classification.is_success());
    assert_eq!(terminal.num_turns, Some(2));
    assert_eq!(
        terminal.result_text.as_deref(),
        Some("rho-tool-fixture-marker-42")
    );
    assert_eq!(
        terminal.session_id.as_deref(),
        Some("22222222-3333-4444-8555-666666666666")
    );
    let usage = terminal.usage.as_ref().expect("usage");
    assert_eq!(usage.input_tokens, Some(4));
    assert_eq!(usage.cache_read_tokens, Some(14452));
    assert_eq!(usage.cache_write_tokens, Some(5604));
    assert_eq!(usage.output_tokens, Some(102));
    assert_eq!(usage.total_input_tokens(), Some(4 + 14452 + 5604));

    let mut status = crate::subagent::RunStatus::default();
    for effect in &effects {
        if let StreamEffect::Status(patch) = effect {
            apply_status_patch(&mut status, patch.clone());
        }
    }
    assert_ne!(status.state, RunState::Ok);
    assert_ne!(status.state, RunState::Error);
    assert_eq!(status.turns, 2);
    assert_eq!(status.input_tokens, 4 + 14452 + 5604);
    assert_eq!(status.output_tokens, 102);
    assert_eq!(
        status.claude_session_id.as_deref(),
        Some("22222222-3333-4444-8555-666666666666")
    );
    assert_eq!(status.result.as_deref(), Some("rho-tool-fixture-marker-42"));
    assert_no_terminal_attachment(&effects);

    // Heartbeat system/status frames from the live capture must stay quiet.
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            StreamEffect::Attachment(AttachmentEvent::Notice(text))
                if text.contains("claude system: status")
        )),
        "live capture must not spam status notices"
    );
}
