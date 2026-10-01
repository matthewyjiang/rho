//! Per-request tool advertisement through [`crate::tool::ToolVisibility`].

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput, ToolVisibility},
    Rho, SessionOptions,
};

/// Advertises `always` and, once `revealed` is set, `revealed_tool`.
struct Gate {
    revealed: AtomicBool,
}

impl ToolVisibility for Gate {
    fn is_advertised(&self, name: &str) -> bool {
        match name {
            "always" => true,
            "revealed_tool" => self.revealed.load(Ordering::SeqCst),
            _ => false,
        }
    }
}

/// Flips `revealed` when called, like a search tool promoting a deferred tool.
struct RevealTool {
    gate: Arc<Gate>,
}

impl crate::tool::Tool for RevealTool {
    fn spec(&self) -> ToolSpec {
        spec("always")
    }

    fn call<'a>(
        &'a self,
        _invocation: crate::tool::ToolInvocation,
        _context: crate::tool::ToolContext,
    ) -> crate::tool::ToolFuture<'a> {
        self.gate.revealed.store(true, Ordering::SeqCst);
        Box::pin(async { Ok(ToolOutput::text("revealed")) })
    }
}

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: name.into(),
        input_schema: json!({"type": "object"}),
    }
}

fn call(id: &str, name: &str) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: json!({}),
        },
    )]))
}

fn names(tools: &[ToolSpec]) -> Vec<String> {
    tools.iter().map(|tool| tool.name.clone()).collect()
}

// Covers: advertisement is chosen per model request (a change mid-run reaches
// the next request), unadvertised tools stay out of the provider tool list,
// and a model call to an unadvertised tool resolves unavailable, not executed.
// Owner: SDK orchestration tool visibility.
#[tokio::test]
async fn visibility_is_resolved_per_request_and_gates_model_calls() {
    let gate = Arc::new(Gate {
        revealed: AtomicBool::new(false),
    });
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [
            // Calls a hidden tool before it is advertised: must not run.
            call("call-1", "revealed_tool"),
            call("call-2", "always"),
            call("call-3", "revealed_tool"),
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                "done".into(),
            )])),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider.clone())
        .tool(RevealTool {
            gate: Arc::clone(&gate),
        })
        .tool(ScriptedTool::new(
            spec("revealed_tool"),
            ScriptedToolOutcome::Success(ToolOutput::text("ran")),
        ))
        .tool(ScriptedTool::new(
            spec("never"),
            ScriptedToolOutcome::Success(ToolOutput::text("never")),
        ))
        .tool_visibility_shared(Arc::clone(&gate) as Arc<dyn ToolVisibility>)
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();

    session.complete("go").await.unwrap();

    let requests = provider.recorded_requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| names(&request.tools))
            .collect::<Vec<_>>(),
        vec![
            vec!["always".to_owned()],
            vec!["always".to_owned()],
            vec!["always".to_owned(), "revealed_tool".to_owned()],
            vec!["always".to_owned(), "revealed_tool".to_owned()],
        ]
    );
    let results = session
        .history()
        .into_iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some((result.ok, result.content)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        results,
        vec![
            (false, "tool 'revealed_tool' is unavailable".to_owned()),
            (true, "revealed".to_owned()),
            (true, "ran".to_owned()),
        ]
    );
}
