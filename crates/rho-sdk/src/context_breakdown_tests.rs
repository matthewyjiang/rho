use pretty_assertions::assert_eq;

use super::ContextItem;
use crate::{
    model::{
        ContentBlock, Message, ModelEvent, ModelIdentity, ModelResponse, ModelUsage, ToolCall,
        ToolResult, ToolSpec,
    },
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput},
    CompactionTrigger, RequestContext, Rho, SessionId, SessionOptions, SystemPrompt,
};

struct FixedContext;

impl RequestContext for FixedContext {
    fn messages(&self, _session_id: &SessionId) -> Vec<Message> {
        vec![Message::model_context("request-only context")]
    }
}

// Covers: breakdown parts drifting from the estimator (sum no longer equals the
// session estimate, or calibration lost), and tool output misattributed when
// results are matched to calls by id, including an orphaned result, or a
// compaction summary counted as user input.
// Owner: SDK context accounting; hosts only aggregate these parts.
#[tokio::test]
async fn breakdown_parts_sum_to_session_estimate_and_attribute_tool_results() {
    let spec = ToolSpec {
        name: "echo".into(),
        description: "returns its result".into(),
        input_schema: serde_json::json!({"type": "object"}),
    };
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall {
                    id: "echo-call".into(),
                    name: spec.name.clone(),
                    arguments: serde_json::json!({}),
                },
            )])),
            ScriptedTurn::streaming(
                vec![ModelEvent::Usage(ModelUsage {
                    input_tokens: Some(1_000),
                    ..ModelUsage::default()
                })],
                ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
            ),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .system_prompt(SystemPrompt::Custom("system prompt".into()))
        .tool(ScriptedTool::new(
            spec,
            ScriptedToolOutcome::Success(ToolOutput::text("tool output")),
        ))
        .request_context(FixedContext)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::new()).await.unwrap();
    session.complete("use the tool").await.unwrap();

    let calibrated = session.context_estimate();
    assert_eq!(calibrated.provider_reported_tokens(), Some(1_000));
    let breakdown = session.context_breakdown();
    assert_eq!(breakdown.estimate(), calibrated);
    assert_eq!(
        breakdown
            .parts()
            .iter()
            .map(|part| part.tokens())
            .sum::<u64>(),
        calibrated.estimated_tokens()
    );

    for message in [
        Message::ToolResult(ToolResult {
            id: "unknown-call".into(),
            ok: true,
            content: "orphan".into(),
        }),
        Message::compaction_summary(CompactionTrigger::Manual, "earlier work"),
    ] {
        session.append_message(message).unwrap();
    }
    let breakdown = session.context_breakdown();
    assert_eq!(
        breakdown
            .parts()
            .iter()
            .map(|part| part.item().clone())
            .collect::<Vec<_>>(),
        vec![
            ContextItem::RequestOverhead,
            ContextItem::ToolSchema {
                name: "echo".into()
            },
            ContextItem::SystemPrompt,
            ContextItem::User,
            ContextItem::Assistant,
            ContextItem::ToolResult {
                tool: Some("echo".into())
            },
            ContextItem::Assistant,
            ContextItem::ToolResult { tool: None },
            ContextItem::CompactionSummary,
            ContextItem::RequestContext,
        ]
    );
}
