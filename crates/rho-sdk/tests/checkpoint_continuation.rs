mod support;

use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelResponse, ToolCall, ToolResult, ToolSpec},
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{Tool, ToolContext, ToolFuture, ToolInvocation, ToolOutput, ToolSecurity},
    CapabilityKind, InMemorySessionStore, Rho, SessionOptions, UserInput,
};
use serde_json::json;
use tokio::sync::mpsc;

use support::{identity, text_response, TEST_TIMEOUT};

/// Records each invocation by name. A stalled tool never finishes on its own.
struct RecordingTool {
    name: &'static str,
    capability: CapabilityKind,
    invoked: mpsc::UnboundedSender<&'static str>,
    stall: bool,
}

impl Tool for RecordingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.into(),
            description: "recording test tool".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([self.capability])
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        self.invoked.send(self.name).unwrap();
        Box::pin(async move {
            if self.stall {
                std::future::pending::<()>().await;
            }
            Ok(ToolOutput::text(format!("{} again", self.name)))
        })
    }
}

fn runtime(
    provider: ScriptedProvider,
    invoked: &mpsc::UnboundedSender<&'static str>,
    stall: bool,
) -> Rho {
    let tool = |name, capability| RecordingTool {
        name,
        capability,
        invoked: invoked.clone(),
        stall,
    };
    Rho::builder()
        .provider(provider)
        .tool(tool("read", CapabilityKind::Read))
        .tool(tool("write", CapabilityKind::Write))
        .build()
        .unwrap()
}

fn call(id: &str, name: &str) -> ContentBlock {
    ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: json!({}),
    })
}

// Covers: a run that dies after a model reply leaves a checkpoint naming the
// reply's tool calls. Continuing it reruns only the read-only call, fails the
// other back to the model, and asks the model again. Owner: SDK checkpoints.
#[tokio::test]
async fn continuation_reruns_read_only_calls_and_interrupts_the_rest() {
    let store = Arc::new(InMemorySessionStore::new());
    let (invoked, mut invocations) = mpsc::unbounded_channel();
    let crashed = runtime(
        ScriptedProvider::new(
            identity(),
            [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                call("read-1", "read"),
                call("write-1", "write"),
            ]))],
        ),
        &invoked,
        /* stall */ true,
    );
    let session = crashed.session(SessionOptions::default()).await.unwrap();
    session.set_checkpoint_store(Some(store.clone())).unwrap();
    let run = session.start(UserInput::text("inspect")).await.unwrap();
    tokio::time::timeout(TEST_TIMEOUT, invocations.recv())
        .await
        .expect("no tool started")
        .unwrap();
    // A dropped event consumer stops the run without a terminal commit, so the
    // store keeps exactly what a crash would leave behind.
    drop(run);
    let saved = store.load(session.id()).expect("checkpoint was saved");
    assert_eq!(
        saved
            .history()
            .last()
            .and_then(Message::completed_assistant_content),
        Some([call("read-1", "read"), call("write-1", "write")].as_slice())
    );

    let provider =
        ScriptedProvider::new(identity(), [ScriptedTurn::completed(text_response("done"))]);
    let (invoked, mut invocations) = mpsc::unbounded_channel();
    let resumed = runtime(provider.clone(), &invoked, /* stall */ false);
    let session = resumed
        .session(SessionOptions::from_snapshot(saved.clone()))
        .await
        .unwrap();
    let mut run = session.continue_history().await.unwrap();
    while run.next_event().await.is_some() {}
    assert_eq!(run.outcome().await.unwrap().text(), "done");

    let mut rerun = Vec::new();
    while let Ok(name) = invocations.try_recv() {
        rerun.push(name);
    }
    assert_eq!(rerun, ["read"]);
    let mut expected = saved.history().to_vec();
    expected.extend([
        Message::ToolResult(ToolResult {
            id: "write-1".into(),
            ok: false,
            content: "tool call interrupted before completion".into(),
        }),
        Message::ToolResult(ToolResult {
            id: "read-1".into(),
            ok: true,
            content: "read again".into(),
        }),
    ]);
    assert_eq!(provider.recorded_requests()[0].messages, expected);
}
