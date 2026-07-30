use std::time::Duration;

use rho_sdk::{
    model::{ModelEvent, ModelRequest, ModelResponse},
    provider::ProviderEventSender,
    ProviderError,
};

use super::{completed_tool_call, fixture_sleep, tool_result};

pub(super) const PROMPT: &str = "fixture apply patch";
pub(super) const CANCEL_PROMPT: &str = "fixture cancel apply patch";
const CALL_ID: &str = "tui-fixture-apply-patch";
const CANCEL_CALL_ID: &str = "tui-fixture-cancel-apply-patch";

pub(super) fn is_pending(request: &ModelRequest<'_>) -> bool {
    tool_result(request, CALL_ID).is_none()
}

pub(super) async fn stream(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    let input = concat!(
        "*** Begin Patch\n",
        "*** Add File: .rho-tui-fixture-patch.txt\n",
        "+streamed patch line\n",
        "*** End Patch",
    );
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: Some("apply_patch".into()),
            arguments: concat!(
                r#"{"input":"*** Begin Patch\n"#,
                r#"*** Add File: .rho-tui-fixture-patch.txt\n"#,
                r#"+streamed patch line\n"#,
            )
            .into(),
        })
        .await?;
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: Some(CALL_ID.into()),
            name: None,
            arguments: String::new(),
        })
        .await?;
    fixture_sleep(&request.cancellation, Duration::from_millis(750)).await?;
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments: r#"*** End Patch"}"#.into(),
        })
        .await?;
    completed_tool_call(CALL_ID, "apply_patch", serde_json::json!({"input": input}))
}

pub(super) async fn stream_until_cancelled(
    request: &ModelRequest<'_>,
    events: &ProviderEventSender,
) -> Result<ModelResponse, ProviderError> {
    let input = concat!(
        "*** Begin Patch\n",
        "*** Add File: .rho-tui-fixture-cancelled-patch.txt\n",
        "+cancelled patch line\n",
        "*** End Patch",
    );
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: None,
            name: Some("apply_patch".into()),
            arguments: concat!(
                r#"{"input":"*** Begin Patch\n"#,
                r#"*** Add File: .rho-tui-fixture-cancelled-patch.txt\n"#,
                r#"+cancelled patch line\n"#,
            )
            .into(),
        })
        .await?;
    events
        .send(ModelEvent::ToolCallDelta {
            index: 0,
            id: Some(CANCEL_CALL_ID.into()),
            name: None,
            arguments: String::new(),
        })
        .await?;
    fixture_sleep(&request.cancellation, Duration::from_secs(30)).await?;
    completed_tool_call(
        CANCEL_CALL_ID,
        "apply_patch",
        serde_json::json!({"input": input}),
    )
}

pub(super) fn completion_text(request: &ModelRequest<'_>) -> Option<String> {
    let result = tool_result(request, CALL_ID)?;
    Some(format!(
        "patch lifecycle complete with one result: {}",
        result.content.lines().next().unwrap_or_default()
    ))
}
