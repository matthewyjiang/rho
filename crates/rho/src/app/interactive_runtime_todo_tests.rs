use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    SessionOptions, UserInput,
};
use serde_json::json;

use super::*;
use crate::{
    agent::{AgentCapabilities, ToolCapability},
    tools::{sdk_registry::ToolSetOptions, todo::TodoList},
};

fn checklist(content: &str) -> TodoList {
    TodoList::parse(json!({"todos": [{"content": content, "status": "in_progress"}]})).unwrap()
}

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        },
    )]))
}

fn text(text: &str) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
        text.into(),
    )]))
}

async fn todo_runtime(
    turns: Vec<ScriptedTurn>,
    storage: Option<StoredSession>,
) -> (InteractiveRuntime, ScriptedProvider) {
    let mut interactive = test_runtime(Vec::new()).await;
    let provider = ScriptedProvider::new(interactive.provider_identity(), turns);
    interactive.provider =
        ProviderController::new(Arc::new(provider.clone()), rho_sdk::ReasoningLevel::Off);
    interactive.compaction.auto_compact = false;
    interactive.tools = AppToolSet::new(
        &interactive.config,
        interactive.diagnostics.clone(),
        ToolSetOptions::new(AgentCapabilities::new(
            [ToolCapability::Todo].into_iter().collect(),
        )),
    );
    let options = storage
        .as_ref()
        .map_or_else(SessionOptions::new, |storage| {
            SessionOptions::new().id(SessionId::from_string(storage.id()).unwrap())
        });
    let session = interactive.runtime.session(options).await.unwrap();
    let snapshot = session.snapshot();
    interactive.sessions = InteractiveSessionController::new(
        session,
        storage,
        interactive.tools.web_access().clone(),
        /*recall*/ None,
        /*advisor*/ None,
    )
    .with_todo_state(interactive.tools.todo_state());
    interactive
        .rebuild_session(
            snapshot,
            ReplacementLifecycle::Rebound,
            SessionWriteRetention::Keep,
            PromptTransition::Keep,
        )
        .await
        .unwrap();
    (interactive, provider)
}

// Covers: rejected replacements, unprinted codemode calls, and a script failure
// after a successful nested todo must not lose or misreport the accepted list.
// Owner: interactive host tool execution and checklist state.
#[tokio::test]
async fn todo_tracks_successful_direct_and_nested_replacements_only() {
    let first = checklist("first\nexact whitespace  ");
    let nested = checklist("nested replacement");
    let (mut interactive, provider) = todo_runtime(
        vec![
            call("first", "todo", serde_json::to_value(&first).unwrap()),
            text("first accepted"),
            call(
                "invalid",
                "todo",
                json!({"todos": [{"content": "", "status": "pending"}]}),
            ),
            text("invalid rejected"),
            call(
                "nested",
                "codemode",
                json!({"script": format!(
                    "call_tool(\"todo\", {})\nfail(\"after successful nested update\")",
                    serde_json::to_string(&nested).unwrap(),
                )}),
            ),
            text("nested accepted despite script failure"),
            call(
                "clear",
                "codemode",
                json!({"script": "call_tool(\"todo\", {\"todos\": []})"}),
            ),
            text("done"),
        ],
        None,
    )
    .await;
    assert_eq!(interactive.todo_list(), None);
    for expected in [first.clone(), first, nested, TodoList { todos: Vec::new() }] {
        interactive
            .start(UserInput::text("track tasks"), None)
            .await
            .unwrap();
        interactive.finish_run().await.unwrap();
        assert_eq!(interactive.todo_list(), Some(expected));
        let requests = provider.recorded_requests();
        let request = requests.last().unwrap();
        assert_eq!(
            request.messages.last(),
            rho_sdk::RequestContext::messages(
                &interactive.tools.todo_state(),
                interactive.sessions.session().id()
            )
            .last(),
        );
    }
    interactive.shutdown().await;
}

// Covers: exact nested state survives real SDK compaction and disk reopen;
// repeated compactions keep request context outside history, clear is durable, and
// selecting a different session or older tree leaf never leaks newer state.
// Owner: interactive host snapshot/compaction and branch lifecycle.
#[tokio::test]
async fn todo_survives_compaction_resume_clear_and_tree_selection() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir(&cwd).unwrap();
    let storage = StoredSession::create_in_root(root.path(), &cwd).unwrap();
    let expected = checklist("keep exact order, punctuation, and whitespace\n  second line  ");
    let updated = checklist("unprinted replacement after compaction");
    let (mut interactive, provider) = todo_runtime(
        vec![
            call(
                "nested",
                "codemode",
                json!({"script": format!(
                    "call_tool(\"todo\", {})", serde_json::to_string(&expected).unwrap(),
                )}),
            ),
            text("done"),
            text("summary without a checklist"),
            text("another summary without a checklist"),
            call(
                "update",
                "codemode",
                json!({"script": format!(
                    "call_tool(\"todo\", {})", serde_json::to_string(&updated).unwrap(),
                )}),
            ),
            text("updated"),
            text("inspected resumed replacement"),
            call("clear", "todo", json!({"todos": []})),
            text("cleared"),
            text("inspected resumed clear"),
        ],
        Some(storage.clone()),
    )
    .await;
    interactive
        .start(UserInput::text("track these tasks"), None)
        .await
        .unwrap();
    interactive.finish_run().await.unwrap();
    assert_eq!(interactive.todo_list(), Some(expected.clone()));
    let before_compaction = crate::session::tree::SessionTree::load(storage.path())
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();
    for _ in 0..2 {
        interactive
            .sessions
            .session()
            .append_message(Message::assistant_text("y".repeat(8_000)))
            .unwrap();
        interactive
            .sessions
            .session()
            .append_message(Message::assistant_text("recent update"))
            .unwrap();
        assert!(interactive.compact().await.unwrap().is_some());
        assert_eq!(interactive.todo_list(), Some(expected.clone()));
        assert_eq!(
            interactive
                .history()
                .iter()
                .filter(|message| message.as_model_context().is_some())
                .count(),
            0
        );
    }
    let compacted_leaf = crate::session::tree::SessionTree::load(storage.path())
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();
    interactive
        .start(UserInput::text("replace tasks after compaction"), None)
        .await
        .unwrap();
    interactive.finish_run().await.unwrap();
    assert_eq!(interactive.todo_list(), Some(updated.clone()));
    let (reopened, _) =
        StoredSession::open_by_id_with_histories_in_root(root.path(), &cwd, storage.id()).unwrap();
    interactive.resume(reopened.clone()).await.unwrap();
    assert_eq!(interactive.todo_list(), Some(updated));
    interactive
        .start(UserInput::text("inspect tasks after resume"), None)
        .await
        .unwrap();
    interactive.finish_run().await.unwrap();
    assert_eq!(
        provider.recorded_requests().last().unwrap().messages.last(),
        rho_sdk::RequestContext::messages(
            &interactive.tools.todo_state(),
            interactive.sessions.session().id()
        )
        .last(),
    );

    interactive
        .start(UserInput::text("clear the checklist"), None)
        .await
        .unwrap();
    interactive.finish_run().await.unwrap();
    assert_eq!(
        interactive.todo_list(),
        Some(TodoList { todos: Vec::new() })
    );
    interactive.resume(reopened.clone()).await.unwrap();
    assert_eq!(
        interactive.todo_list(),
        Some(TodoList { todos: Vec::new() })
    );
    interactive
        .start(UserInput::text("inspect cleared tasks after resume"), None)
        .await
        .unwrap();
    interactive.finish_run().await.unwrap();
    assert_eq!(
        provider.recorded_requests().last().unwrap().messages.last(),
        rho_sdk::RequestContext::messages(
            &interactive.tools.todo_state(),
            interactive.sessions.session().id()
        )
        .last(),
    );
    interactive
        .select_tree_node(reopened.clone(), &compacted_leaf)
        .await
        .unwrap();
    assert_eq!(interactive.todo_list(), Some(expected.clone()));
    interactive
        .select_tree_node(reopened.clone(), &before_compaction)
        .await
        .unwrap();
    assert_eq!(interactive.todo_list(), Some(expected.clone()));

    let other = StoredSession::create_in_root(root.path(), &cwd).unwrap();
    interactive.resume(other).await.unwrap();
    assert_eq!(interactive.todo_list(), None);
    interactive.resume(reopened).await.unwrap();
    assert_eq!(interactive.todo_list(), Some(expected));
    interactive.reset().await.unwrap();
    assert_eq!(interactive.todo_list(), None);
    interactive.shutdown().await;
}
