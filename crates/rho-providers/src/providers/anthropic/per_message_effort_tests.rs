use pretty_assertions::assert_eq;
use serde_json::json;

use super::{apply, beta_header};
use crate::model::ReasoningShift;
use crate::protocol::anthropic_messages::{
    AnthropicContentBlock, AnthropicMessage, AnthropicOutputConfig, AnthropicRole,
    PlacedReasoningShift,
};
use crate::reasoning::ReasoningLevel::{self, High, Low, Max, Medium, Minimal, Off, Xhigh};

const FABLE_5_1: &str = "claude-fable-5-1";
const FABLE_5: &str = "claude-fable-5";

fn effort(name: &'static str) -> Option<AnthropicOutputConfig> {
    Some(AnthropicOutputConfig { effort: name })
}

fn effort_for(level: ReasoningLevel) -> Option<&'static str> {
    match level {
        Off => None,
        Minimal | Low => Some("low"),
        Medium => Some("medium"),
        High | Xhigh | Max => Some("high"),
    }
}

fn text(role: AnthropicRole, text: &str) -> AnthropicMessage {
    AnthropicMessage::new(
        role,
        vec![AnthropicContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
    )
}

fn tool_result(id: &str) -> AnthropicMessage {
    AnthropicMessage::new(
        AnthropicRole::User,
        vec![AnthropicContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: "ok".into(),
            is_error: false,
            cache_control: None,
        }],
    )
}

/// user, assistant, user, assistant, user
fn conversation() -> Vec<AnthropicMessage> {
    vec![
        text(AnthropicRole::User, "1"),
        text(AnthropicRole::Assistant, "ok"),
        text(AnthropicRole::User, "2"),
        text(AnthropicRole::Assistant, "ok"),
        text(AnthropicRole::User, "3"),
    ]
}

fn shift(at: usize, from: ReasoningLevel, to: ReasoningLevel) -> PlacedReasoningShift {
    PlacedReasoningShift {
        at,
        shift: ReasoningShift { from, to },
    }
}

/// Role and marker effort of each message, for whole-request comparison.
fn shape(messages: &[AnthropicMessage]) -> Vec<(AnthropicRole, Option<&'static str>)> {
    messages
        .iter()
        .map(|message| {
            (
                message.role,
                message.output_config.as_ref().map(|config| config.effort),
            )
        })
        .collect()
}

// Covers: history shifts keep the starting effort top-level and pin markers
// before the user turn they apply to; shifts that cannot take a marker,
// out-of-band request levels, Off, and unsupported models fall back.
// Owner: anthropic per-message effort
#[test]
fn history_shifts_pin_effort_markers_or_fall_back() {
    use AnthropicRole::{Assistant as A, System as S, User as U};
    let mut on_tool_result = conversation();
    on_tool_result[4] = tool_result("toolu_1");
    let cases = [
        (
            "no shifts",
            FABLE_5_1,
            conversation(),
            vec![],
            effort("high"),
            effort("high"),
            vec![(U, None), (A, None), (U, None), (A, None), (U, None)],
        ),
        (
            "one shift",
            FABLE_5_1,
            conversation(),
            vec![shift(2, High, Low)],
            effort("low"),
            effort("high"),
            vec![
                (U, None),
                (A, None),
                (S, Some("low")),
                (U, None),
                (A, None),
                (U, None),
            ],
        ),
        (
            "two shifts, one without an effort change",
            FABLE_5_1,
            conversation(),
            vec![shift(2, High, Xhigh), shift(4, Xhigh, Medium)],
            effort("medium"),
            effort("high"),
            vec![
                (U, None),
                (A, None),
                (U, None),
                (A, None),
                (S, Some("medium")),
                (U, None),
            ],
        ),
        (
            "shift on a tool-result turn restarts the prefix",
            FABLE_5_1,
            on_tool_result.clone(),
            vec![shift(2, High, Low), shift(4, Low, Medium)],
            effort("medium"),
            effort("medium"),
            vec![(U, None), (A, None), (U, None), (A, None), (U, None)],
        ),
        (
            "request level differs from history",
            FABLE_5_1,
            conversation(),
            vec![shift(2, High, Low)],
            effort("medium"),
            effort("medium"),
            vec![(U, None), (A, None), (U, None), (A, None), (U, None)],
        ),
        (
            "conversation started with Off",
            FABLE_5_1,
            conversation(),
            vec![shift(2, Off, Low)],
            effort("low"),
            effort("low"),
            vec![(U, None), (A, None), (U, None), (A, None), (U, None)],
        ),
        (
            "model without per-message effort",
            FABLE_5,
            conversation(),
            vec![shift(2, High, Low)],
            effort("low"),
            effort("low"),
            vec![(U, None), (A, None), (U, None), (A, None), (U, None)],
        ),
    ];
    for (name, model, mut messages, shifts, current, expected_top, expected_shape) in cases {
        let top = apply(model, current, &shifts, effort_for, &mut messages);
        assert_eq!(top, expected_top, "{name}");
        assert_eq!(shape(&messages), expected_shape, "{name}");
    }
}

// Covers: the marker is an empty effort-only system message behind the beta.
// Owner: anthropic per-message effort
#[test]
fn effort_marker_serializes_as_empty_system_message_behind_beta() {
    let mut messages = conversation();
    apply(
        FABLE_5_1,
        effort("low"),
        &[shift(2, High, Low)],
        effort_for,
        &mut messages,
    );
    assert_eq!(beta_header(&messages), Some(super::BETA));
    assert_eq!(
        serde_json::to_value(&messages[2]).unwrap(),
        json!({"role": "system", "content": [], "output_config": {"effort": "low"}})
    );
    assert_eq!(beta_header(&conversation()), None);
}
