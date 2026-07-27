use pretty_assertions::assert_eq;

use crate::{run_artifacts::AttachmentEvent, subagent::RunState};

use super::stream_test_support::*;
use super::*;

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
    assert_eq!(status.input_tokens, Some(2 + 7294 + 6095));
    assert_eq!(status.output_tokens, Some(4));
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
    assert_eq!(status.input_tokens, Some(2 + 3289 + 5413));
    assert_eq!(status.output_tokens, Some(14));
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
        StreamEffect::Attachment(AttachmentEvent::ToolStarted { card, .. }) => {
            Some(vec![card.header_text()])
        }
        _ => None,
    });
    let started = started.expect("tool started");
    assert!(started[0].contains("Read"));
    let finished = effects.iter().any(|effect| {
        matches!(
            effect,
            StreamEffect::Attachment(AttachmentEvent::ToolFinished { card, .. }) if card.status == rho_tools::tool_card::ToolStatus::Ok
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
        "usage":{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":20,"cache_creation_input_tokens":5},
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
    // Top-level total (10+20+5=35) differs from every modelUsage entry total
    // (z-model=9, a-model=1+3+4=8) and from their sum (17).
    assert_eq!(
        terminal
            .usage
            .as_ref()
            .and_then(ModelUsage::total_input_tokens),
        Some(10 + 20 + 5)
    );
    assert_eq!(
        terminal.context.as_ref().and_then(|context| context.tokens),
        Some(10 + 20 + 5)
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
        StreamEffect::Attachment(AttachmentEvent::ToolStarted { card, .. }) => {
            Some(card.header_text())
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
fn system_thinking_tokens_heartbeat_is_quiet() {
    let effects =
        map_line(r#"{"type":"system","subtype":"thinking_tokens","session_id":"sess-think"}"#);
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, StreamEffect::Attachment(AttachmentEvent::Notice(_)))),
        "thinking_tokens must not spam notices: {effects:?}"
    );
    assert!(effects.iter().any(|effect| matches!(
        effect,
        StreamEffect::Status(StatusPatch {
            claude_session_id: Some(id),
            ..
        }) if id == "sess-think"
    )));
}
