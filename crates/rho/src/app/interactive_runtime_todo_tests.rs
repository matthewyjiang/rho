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

// Covers: generated nested todo updates must be rejected before committing
// uncompactable mandatory state, and the provider must remain reachable.
// Owner: host checklist capacity at the shared direct/nested execution seam.
#[tokio::test]
async fn oversized_nested_todo_keeps_the_previous_list_and_provider_reachable() {
    let accepted = checklist("keep this task");
    let (mut interactive, provider) = todo_runtime(
        vec![
            call("initial", "todo", serde_json::to_value(&accepted).unwrap()),
            text("accepted"),
            call("oversized", "codemode", json!({"script":
                "print(call_tool(\"todo\", {\"todos\": [{\"content\": \"x\" * 1000000, \"status\": \"pending\"}]}))"
            })),
            text("still reachable"),
            text("normal next turn"),
        ],
        None,
    ).await;
    let capacity = rho_sdk::model::context::estimate_context_tokens(
        &interactive.history(),
        &interactive.tools.specs(),
    ) * 2;
    interactive.set_context_window(Some(capacity)).unwrap();
    for _ in 0..3 {
        interactive
            .start(UserInput::text("continue"), None)
            .await
            .unwrap();
        interactive.finish_run().await.unwrap();
        assert_eq!(interactive.todo_list(), Some(accepted.clone()));
    }
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 5);
    let nested = requests[3]
        .messages
        .iter()
        .find_map(|message| match message {
            Message::ToolResult(result) if result.id == "oversized" => Some(result),
            _ => None,
        })
        .unwrap();
    assert!(!nested.ok);
    // The nested ToolError is serialized inside the codemode traceback. Check
    // that wire diagnostic, not generic failure (which could hide a script cap).
    assert!(
        nested.content.contains(&format!(
            "todo mandatory context budget exceeded: limit {capacity} estimated tokens, asked "
        )),
        "{}",
        nested.content
    );
    interactive.shutdown().await;
}

// Covers: request admission includes incoming input and refreshes on headless
// turns as well as interactive turns; startup history is not a request budget.
// Owner: host checklist policy at the shared SDK request boundary.
#[tokio::test]
async fn todo_budget_includes_incoming_input_on_interactive_and_headless_turns() {
    use rho_sdk::{
        model::context::{estimate_context_tokens, estimate_messages_tokens},
        RequestContext,
    };

    for interactive_host in [true, false] {
        let generated = checklist(&"x".repeat(4_096));
        let (mut interactive, provider) = todo_runtime(
            vec![
                text("seed history"),
                call("proposal", "codemode", json!({"script":
                    "call_tool(\"todo\", {\"todos\": [{\"content\": \"x\" * 4096, \"status\": \"in_progress\"}]})"
                })),
                text("replacement rejected"),
            ],
            None,
        ).await;
        interactive
            .sessions
            .session()
            .complete("seed")
            .await
            .unwrap();
        let measured = crate::tools::todo::TodoState::default();
        measured.replace(Some(generated));
        let projection = measured.messages(interactive.sessions.session().id());
        // Fits the previous turn exactly, but not the incoming paste. The
        // capacity comes from the actual serialized history and projection.
        let capacity = estimate_context_tokens(&interactive.history(), &interactive.tools.specs())
            + estimate_messages_tokens(&projection);
        interactive.set_context_window(Some(capacity)).unwrap();
        let incoming = "new input ".repeat(200);
        if interactive_host {
            interactive
                .start(UserInput::text(incoming), None)
                .await
                .unwrap();
            interactive.finish_run().await.unwrap();
        } else {
            // ACP and automation use the same SDK session entry directly.
            interactive
                .sessions
                .session()
                .complete(incoming)
                .await
                .unwrap();
        }
        assert_eq!(interactive.todo_list(), None);
        let requests = provider.recorded_requests();
        let result = requests
            .last()
            .unwrap()
            .messages
            .iter()
            .find_map(|message| match message {
                Message::ToolResult(result) if result.id == "proposal" => Some(result),
                _ => None,
            })
            .unwrap();
        assert!(!result.ok);
        assert!(
            result
                .content
                .contains("todo mandatory context budget exceeded"),
            "{}",
            result.content
        );
        interactive.shutdown().await;
    }
}

// Covers: a generated list that fits the prompt/schema budget but crowds out
// real history must not strand compaction or reactivate on the next turn.
// Owner: host runtime compaction and checklist recovery, not TUI rendering.
#[tokio::test]
async fn boundary_nested_todo_compaction_preserves_state_and_provider_recovery() {
    use rho_sdk::{
        model::context::{estimate_context_tokens, estimate_messages_tokens},
        RequestContext,
    };

    let generated = checklist(&"x".repeat(4_096));
    let (mut interactive, provider) = todo_runtime(
        vec![
            call("boundary", "codemode", json!({"script":
                "call_tool(\"todo\", {\"todos\": [{\"content\": \"x\" * 4096, \"status\": \"in_progress\"}]})"
            })),
            text("accepted"),
            text("brief summary"),
            call("clear", "todo", json!({"todos": []})),
            text("recovered"),
        ],
        None,
    ).await;
    let measured = crate::tools::todo::TodoState::default();
    measured.replace(Some(generated.clone()));
    let full = measured.messages(interactive.sessions.session().id());
    // Exactly fits the initial request, including accepted input: no invented
    // reserve. Later tool results and output crowd out the admitted projection.
    let capacity = estimate_context_tokens(&interactive.history(), &interactive.tools.specs())
        + estimate_messages_tokens(&[Message::user_text("track tasks")])
        + estimate_messages_tokens(&full);
    interactive.set_context_window(Some(capacity)).unwrap();
    interactive
        .start(UserInput::text("track tasks"), None)
        .await
        .unwrap();
    interactive.finish_run().await.unwrap();
    assert_eq!(interactive.todo_list(), Some(generated.clone()));

    // Even after summarizing the old tool group, the actual replacement needs
    // space beyond the prompt/schema-only admission budget.
    interactive.compact().await.unwrap();
    assert_eq!(provider.recorded_requests().len(), 3);
    assert_eq!(interactive.todo_list(), Some(generated.clone()));
    let snapshot = interactive.sessions.session().snapshot();
    let reader = crate::tools::todo::TodoState::default();
    reader.restore(&snapshot);
    assert_eq!(reader.list(), Some(generated));
    let recovery = interactive
        .tools
        .todo_state()
        .messages(snapshot.session_id());
    assert_ne!(recovery, full);
    assert_eq!(recovery.len(), 1);
    assert!(
        estimate_context_tokens(snapshot.history(), &interactive.tools.specs())
            + estimate_messages_tokens(&recovery)
            <= capacity
    );

    interactive
        .start(UserInput::text("clear tasks"), None)
        .await
        .unwrap();
    interactive.finish_run().await.unwrap();
    let requests = provider.recorded_requests();
    assert_ne!(requests[3].messages.last(), full.last());
    assert!(estimate_context_tokens(&requests[3].messages, &requests[3].tools) <= capacity);
    assert_eq!(
        interactive.todo_list(),
        Some(TodoList { todos: Vec::new() })
    );
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
