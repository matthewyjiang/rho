//! Exact-match streaming prompt table for matrix mode.

use std::time::Duration;

use rho_sdk::{
    model::{ContentBlock, ModelEvent, ModelRequest, ModelResponse, ToolCall},
    provider::ProviderEventSender,
    ProviderError, ProviderErrorKind, Retryability,
};

use super::{
    completed, completed_tool_call, fixture_sleep, tool_result, AGENTS_LIST_CALL_ID,
    BACKGROUND_AGENT_CALL_ID, BACKGROUND_CLAUDE_AGENT_CALL_ID,
    BACKGROUND_QUESTIONNAIRE_AGENT_CALL_ID, CLAUDE_AGENT_CALL_ID, CLAUDE_AGENT_ERROR_CALL_ID,
    CONCURRENT_FAST_CALL_ID, CONCURRENT_SLOW_CALL_ID, HOVER_TOOL_CALL_ID, LONG_APPROVAL_CALL_ID,
    PROCESS_RAIL_CALL_ID, PROGRESS_CALL_ID, QUESTIONNAIRE_CALL_ID, SUBAGENT_RAIL_AGENT_CALL_ID,
    TOOL_CALL_ID,
};

pub(super) async fn intercept(
    prompt: &str,
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Option<Result<ModelResponse, ProviderError>> {
    match prompt {
        "fixture slow stream" => {
            if let Err(error) = fixture_sleep(&request.cancellation, Duration::from_secs(4)).await {
                return Some(Err(error));
            }
            Some(completed("assistant stream part one part two"))
        }
        "fixture stream" => Some(stream_reasoning(request, events).await),
        "fixture interleaved reasoning" => Some(stream_interleaved_reasoning(events).await),
        "fixture markdown headings" => Some(stream_markdown_headings(request, events).await),
        // Stable prose must stay drawn while later emphasis markers complete.
        "fixture markdown emphasis stream" => Some(stream_markdown_emphasis(request, events).await),
        "fixture mermaid flowchart" => Some(stream_mermaid(request, events).await),
        "fixture approval long" if tool_result(request, LONG_APPROVAL_CALL_ID).is_none() => {
            Some(approval_long())
        }
        "fixture tool" if tool_result(request, TOOL_CALL_ID).is_none() => {
            Some(stream_write_tool(request, events).await)
        }
        // Long write body so the collapsed card truncates and becomes
        // click-toggleable; used by the tool-card hover lift scenario.
        "fixture hover tool" if tool_result(request, HOVER_TOOL_CALL_ID).is_none() => {
            Some(hover_tool())
        }
        "fixture questionnaire" if tool_result(request, QUESTIONNAIRE_CALL_ID).is_none() => {
            Some(questionnaire_call())
        }
        "fixture concurrent progress"
            if tool_result(request, CONCURRENT_SLOW_CALL_ID).is_none()
                && tool_result(request, CONCURRENT_FAST_CALL_ID).is_none() =>
        {
            Some(concurrent_progress())
        }
        "fixture progress tool" if tool_result(request, PROGRESS_CALL_ID).is_none() => {
            Some(stream_progress_tool(request, events).await)
        }
        "fixture process rail" if tool_result(request, PROCESS_RAIL_CALL_ID).is_none() => {
            Some(completed_tool_call(
                PROCESS_RAIL_CALL_ID,
                "process",
                serde_json::json!({
                    "action": "start",
                    "command": "sleep 60",
                }),
            ))
        }
        "fixture subagent rail" if tool_result(request, SUBAGENT_RAIL_AGENT_CALL_ID).is_none() => {
            Some(completed_tool_call(
                SUBAGENT_RAIL_AGENT_CALL_ID,
                "agent",
                serde_json::json!({
                    "agent_id": "worker",
                    "prompt": "fixture delay",
                    "background": true,
                }),
            ))
        }
        "fixture background agent" if tool_result(request, BACKGROUND_AGENT_CALL_ID).is_none() => {
            Some(stream_background_agent(request, events).await)
        }
        "fixture claude agent" if tool_result(request, CLAUDE_AGENT_CALL_ID).is_none() => {
            // Foreground delegation into a runtime: claude-cli agent definition.
            // The PTY E2E installs that definition and a fake `claude` on PATH.
            Some(completed_tool_call(
                CLAUDE_AGENT_CALL_ID,
                "agent",
                serde_json::json!({
                    "agent_id": "claude-planner",
                    "prompt": "Say hello in one short sentence.",
                    "background": false,
                }),
            ))
        }
        "fixture background claude agent"
            if tool_result(request, BACKGROUND_CLAUDE_AGENT_CALL_ID).is_none() =>
        {
            // Background Claude run so terminal cost lands through automatic
            // completion delivery rather than the foreground tool result path.
            Some(completed_tool_call(
                BACKGROUND_CLAUDE_AGENT_CALL_ID,
                "agent",
                serde_json::json!({
                    "agent_id": "claude-planner",
                    "prompt": "Say hello in one short sentence.",
                    "background": true,
                }),
            ))
        }
        "fixture claude agent error"
            if tool_result(request, CLAUDE_AGENT_ERROR_CALL_ID).is_none() =>
        {
            Some(completed_tool_call(
                CLAUDE_AGENT_ERROR_CALL_ID,
                "agent",
                serde_json::json!({
                    "agent_id": "claude-planner",
                    "prompt": "Force a deterministic Claude error path.",
                    "background": false,
                }),
            ))
        }
        "fixture background questionnaire"
            if tool_result(request, BACKGROUND_QUESTIONNAIRE_AGENT_CALL_ID).is_none() =>
        {
            Some(completed_tool_call(
                BACKGROUND_QUESTIONNAIRE_AGENT_CALL_ID,
                "agent",
                serde_json::json!({
                    "agent_id": "worker",
                    "prompt": "fixture child questionnaire",
                    "background": true,
                }),
            ))
        }
        "fixture child questionnaire" | "fixture delayed child questionnaire"
            if tool_result(request, QUESTIONNAIRE_CALL_ID).is_none() =>
        {
            Some(child_questionnaire(prompt, request).await)
        }
        "fixture agents list" if tool_result(request, AGENTS_LIST_CALL_ID).is_none() => {
            Some(completed_tool_call(
                AGENTS_LIST_CALL_ID,
                "agents",
                serde_json::json!({"action": "list"}),
            ))
        }
        "fixture steering" => Some(stream_steering(request, events).await),
        "fixture delay" => Some(stream_delay(request, events).await),
        "fixture input flood" => Some(stream_input_flood(request, events).await),
        "fixture checkpointed input flood" => {
            Some(stream_checkpointed_input_flood(request, events).await)
        }
        "fixture scroll checkpoint" => Some(stream_scroll_checkpoint(request, events).await),
        "fixture stream failure" => Some(stream_failure(request, events).await),
        "fixture bulk one" | "fixture bulk two" => Some(stream_bulk(prompt, events).await),
        _ => None,
    }
}

async fn stream_reasoning(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    events
        .send(ModelEvent::ReasoningDelta(
            "deterministic reasoning phase one\n".into(),
        ))
        .await?;
    fixture_sleep(&request.cancellation, Duration::from_millis(250)).await?;
    events
        .send(ModelEvent::ReasoningDelta(
            "deterministic reasoning phase two\n".into(),
        ))
        .await?;
    events
        .send(ModelEvent::OutputDelta("assistant stream part one ".into()))
        .await?;
    fixture_sleep(&request.cancellation, Duration::from_millis(350)).await?;
    events
        .send(ModelEvent::OutputDelta("part two".into()))
        .await?;
    completed("assistant stream part one part two")
}

async fn stream_interleaved_reasoning(
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    for event in [
        ModelEvent::OutputDelta("assistant before reasoning\n".into()),
        ModelEvent::ReasoningDelta("deterministic reasoning phase one\n".into()),
        ModelEvent::ReasoningDelta("deterministic reasoning phase two\n".into()),
        ModelEvent::OutputDelta("assistant after reasoning".into()),
    ] {
        events.send(event).await?;
    }
    completed("assistant before reasoning\nassistant after reasoning")
}

async fn stream_markdown_headings(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    stream_paused_deltas(
        request,
        events,
        [
            ("# Level one\n## Lev", 40),
            ("el two\n### Level three\n", 40),
            ("#### Level four\n##### Lev", 40),
            ("el five\n###### Level six", 40),
        ],
    )
    .await
}

async fn stream_markdown_emphasis(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    // Hold the open-emphasis delta longer so PTY scenarios can sample
    // ALPHA staying visible before BETA arrives. CI runners under load
    // miss a shorter window when the PTY poll thread is starved.
    stream_paused_deltas(
        request,
        events,
        [
            ("Stable prose ALPHA remains drawn ", 250),
            ("while **hold", 1500),
            ("ing closes** and trailing BETA completes.", 250),
        ],
    )
    .await
}

async fn stream_mermaid(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    // Hold the last open-fence body so PTY can sample live art before
    // the closing fence arrives.
    stream_paused_deltas(
        request,
        events,
        [
            ("```mermaid\nflowchart LR\n", 60),
            (
                "  P1[\"Phase 1: retention sweep\"] --> P2[\"Phase 2: parent link on disk\"]\n",
                60,
            ),
            ("  P2 --> P3[\"Phase 3: session delete API + CLI\"]\n", 60),
            (
                "  P3 --> P4[\"Phase 4: TUI delete in resume picker\"]\n",
                60,
            ),
            ("  P3 --> P5[\"Phase 5: nest runs under session\"]\n", 500),
            ("```\ndiagram delivered", 60),
        ],
    )
    .await
}

async fn stream_paused_deltas<const N: usize>(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
    deltas: [(&str, u64); N],
) -> Result<ModelResponse, ProviderError> {
    let mut response = String::new();
    for (delta, pause_ms) in deltas {
        events.send(ModelEvent::OutputDelta(delta.into())).await?;
        response.push_str(delta);
        fixture_sleep(&request.cancellation, Duration::from_millis(pause_ms)).await?;
    }
    completed(response)
}

fn approval_long() -> Result<ModelResponse, ProviderError> {
    // Prefix stays long enough that common PTY widths bury the suffix
    // below the first approval detail page under command-first layout.
    let mut command = String::from("printf 'reviewing harmless fixture'; printf '");
    for index in 1..=40 {
        command.push_str(&format!("segment-{index:02} "));
    }
    command.push_str("'; echo DANGEROUS_SUFFIX_INSPECTABLE");
    completed_tool_call(
        LONG_APPROVAL_CALL_ID,
        "bash",
        serde_json::json!({ "command": command }),
    )
}

async fn stream_write_tool(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    let arguments = serde_json::json!({
        "path": ".rho-tui-fixture-output.txt",
        "content": "deterministic tool output\n",
    });
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: Some(TOOL_CALL_ID.into()),
            name: Some("write".into()),
            arguments: "{\"path\":\".rho-tui-fixture-output.txt\",".into(),
        })
        .await?;
    fixture_sleep(&request.cancellation, Duration::from_millis(300)).await?;
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments: "\"content\":\"deterministic tool output\\n\"}".into(),
        })
        .await?;
    completed_tool_call(TOOL_CALL_ID, "write", arguments)
}

fn hover_tool() -> Result<ModelResponse, ProviderError> {
    let content = (1..=40)
        .map(|line| format!("hover fixture line {line:02}\n"))
        .collect::<String>();
    let arguments = serde_json::json!({
        "path": ".rho-tui-fixture-hover.txt",
        "content": content,
    });
    completed_tool_call(HOVER_TOOL_CALL_ID, "write", arguments)
}

fn questionnaire_call() -> Result<ModelResponse, ProviderError> {
    completed_tool_call(
        QUESTIONNAIRE_CALL_ID,
        "questionnaire",
        serde_json::json!({
            "title": "Deterministic questionnaire",
            "reason": "Validate exactly-once host input delivery.",
            "questions": [{
                "id": "color",
                "question": "Choose one color",
                "type": "choice",
                "choices": [
                    {
                        "label": "red",
                        "description": "A warm primary color"
                    },
                    {
                        "label": "blue",
                        "description": "A cool primary color"
                    }
                ],
                "default": "red",
                "required": true,
            }],
        }),
    )
}

fn concurrent_progress() -> Result<ModelResponse, ProviderError> {
    Ok(ModelResponse::Assistant(vec![
        ContentBlock::ToolCall(ToolCall {
            id: CONCURRENT_SLOW_CALL_ID.into(),
            name: "tui_fixture_progress".into(),
            arguments: serde_json::json!({"label": "slow fixture", "delay_ms": 2500}),
        }),
        ContentBlock::ToolCall(ToolCall {
            id: CONCURRENT_FAST_CALL_ID.into(),
            name: "tui_fixture_progress".into(),
            arguments: serde_json::json!({"label": "fast fixture", "delay_ms": 200}),
        }),
    ]))
}

async fn stream_progress_tool(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: Some(PROGRESS_CALL_ID.into()),
            name: Some("tui_fixture_progress".into()),
            arguments: "{}".into(),
        })
        .await?;
    fixture_sleep(&request.cancellation, Duration::from_millis(500)).await?;
    completed_tool_call(
        PROGRESS_CALL_ID,
        "tui_fixture_progress",
        serde_json::json!({}),
    )
}

async fn stream_background_agent(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: Some(BACKGROUND_AGENT_CALL_ID.into()),
            name: Some("agent".into()),
            arguments: r#"{"agent_id":"wor"#.into(),
        })
        .await?;
    fixture_sleep(&request.cancellation, Duration::from_millis(250)).await?;
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments: r#"ker","prompt":"fixture stream","background":true}"#.into(),
        })
        .await?;
    completed_tool_call(
        BACKGROUND_AGENT_CALL_ID,
        "agent",
        serde_json::json!({
            "agent_id": "worker",
            "prompt": "fixture stream",
            "background": true,
        }),
    )
}

async fn child_questionnaire(
    prompt: &str,
    request: &ModelRequest<'_>,
) -> Result<ModelResponse, ProviderError> {
    if prompt == "fixture delayed child questionnaire" {
        fixture_sleep(&request.cancellation, Duration::from_secs(1)).await?;
    }
    completed_tool_call(
        QUESTIONNAIRE_CALL_ID,
        "questionnaire",
        serde_json::json!({
            "title": "Background questionnaire",
            "reason": "Validate delegated host input routing.",
            "questions": [{
                "id": "color",
                "question": "Choose one color",
                "type": "choice",
                "choices": [
                    {
                        "label": "red",
                        "description": "A warm primary color"
                    },
                    {
                        "label": "blue",
                        "description": "A cool primary color"
                    }
                ],
                "default": "red",
                "required": true,
            }],
        }),
    )
}

async fn stream_steering(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    events
        .send(ModelEvent::OutputDelta(
            "initial turn waiting for steering".into(),
        ))
        .await?;
    fixture_sleep(&request.cancellation, Duration::from_secs(2)).await?;
    completed("initial turn waiting for steering")
}

async fn stream_delay(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    events
        .send(ModelEvent::OutputDelta(
            "partial assistant before cancellation".into(),
        ))
        .await?;
    fixture_sleep(&request.cancellation, Duration::from_secs(30)).await?;
    completed("delay unexpectedly completed")
}

async fn stream_input_flood(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    let mut response = String::new();
    for index in 1..=400 {
        let chunk = format!("input flood event {index:03}\n");
        events.send(ModelEvent::OutputDelta(chunk.clone())).await?;
        response.push_str(&chunk);
        fixture_sleep(&request.cancellation, Duration::from_millis(5)).await?;
    }
    // Keep the turn live for scenarios using the ungated flood.
    fixture_sleep(&request.cancellation, Duration::from_secs(30)).await?;
    completed(response)
}

async fn stream_checkpointed_input_flood(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    // Mirrored in rho-tui-pty/src/scenarios/type_during_stream.rs.
    const RELEASE_MARKER: &str = ".rho-fixture-release-input-flood";
    // Clear a cancelled run's release before any output acknowledges readiness.
    // Never clear at a checkpoint: the harness may already have released it.
    super::release::consume_release(RELEASE_MARKER)?;
    // Preserve the original 400-delta workload; split it at startup and halfway
    // so overlay input and draft input each get a separately released flood.
    for batch in [1..=10, 11..=200, 201..=400] {
        let last = *batch.end();
        for index in batch {
            events
                .send(ModelEvent::OutputDelta(format!(
                    "input flood event {index:03}\n"
                )))
                .await?;
            // Stream pacing only. Checkpoint receipts, not this delay, sync input.
            fixture_sleep(&request.cancellation, Duration::from_millis(5)).await?;
        }
        if last != 400 {
            super::release::wait_for_release_or_cancel(RELEASE_MARKER, &request.cancellation)
                .await?;
        }
    }
    // Only the scenario's empty-composer Esc can finish this turn.
    request.cancellation.cancelled().await;
    Err(ProviderError::interrupted("input flood stopped"))
}

async fn stream_scroll_checkpoint(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    let response = (1..=100)
        .map(|index| format!("scroll checkpoint event {index:03}\n"))
        .collect::<String>();
    events.send(ModelEvent::OutputDelta(response)).await?;
    fixture_sleep(&request.cancellation, Duration::from_secs(30)).await?;
    completed("scroll checkpoint unexpectedly completed")
}

async fn stream_failure(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    events
        .send(ModelEvent::OutputDelta(
            "partial assistant before forced stream termination".into(),
        ))
        .await?;
    fixture_sleep(&request.cancellation, Duration::from_millis(300)).await?;
    Err(ProviderError::new(
        ProviderErrorKind::Other,
        "deterministic forced stream termination",
        Retryability::Permanent,
    ))
}

async fn stream_bulk(
    prompt: &str,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    let response = bulk_response(prompt);
    events
        .send(ModelEvent::OutputDelta(response.clone()))
        .await?;
    completed(response)
}

fn bulk_response(prompt: &str) -> String {
    (1..=180)
        .map(|line| {
            format!(
                "{prompt} line {line:03}: deterministic transcript payload {}\n",
                "x".repeat(64)
            )
        })
        .collect()
}
