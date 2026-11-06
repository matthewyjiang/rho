use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    PlanEntry, PlanEntryPriority, PlanEntryStatus, SessionUpdate,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{Message, ModelIdentity},
    provider::ScriptedProvider,
    RunEvent, SessionOptions, ToolCallId, ToolCompletion,
};

use super::SessionSignals;
use crate::{
    app::session_runtime::ContextGauge,
    tools::todo::{TodoItem, TodoList, TodoState, TodoStatus},
};

async fn session() -> rho_sdk::Session {
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::new(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "model"),
            Vec::new(),
        )))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::new()).await.unwrap();
    session
        .append_message(Message::user_text("hello there"))
        .unwrap();
    session
}

fn list(items: &[(&str, TodoStatus)]) -> Option<TodoList> {
    Some(TodoList {
        todos: items
            .iter()
            .map(|(content, status)| TodoItem {
                content: (*content).into(),
                status: *status,
            })
            .collect(),
    })
}

fn entry(content: &str, status: PlanEntryStatus) -> PlanEntry {
    PlanEntry::new(content, PlanEntryPriority::Medium, status)
}

fn tool_finished() -> RunEvent {
    RunEvent::ToolFinished {
        call_id: ToolCallId::from_string("call-1").unwrap(),
        result: ToolCompletion::Unavailable,
    }
}

// Covers: the todo checklist reaches ACP clients as a full `plan` only when it
// differs from what the client last saw, including a checklist restored by
// load that the client has never received.
// Owner: ACP session signals
#[tokio::test]
async fn finished_tools_send_the_checklist_only_when_it_changed() {
    let todo = TodoState::default();
    todo.replace(list(&[("read", TodoStatus::Completed)]));
    let mut signals = SessionSignals::new(
        ContextGauge::of_session_for_tests(session().await, /*configured_window*/ None),
        todo.clone(),
        Vec::new(),
    );
    let steps = [
        (
            "restored list the client has not seen",
            list(&[("read", TodoStatus::Completed)]),
            Some(vec![entry("read", PlanEntryStatus::Completed)]),
        ),
        (
            "replaced list",
            list(&[
                ("read", TodoStatus::Completed),
                ("edit", TodoStatus::InProgress),
            ]),
            Some(vec![
                entry("read", PlanEntryStatus::Completed),
                entry("edit", PlanEntryStatus::InProgress),
            ]),
        ),
        (
            "same list again",
            list(&[
                ("read", TodoStatus::Completed),
                ("edit", TodoStatus::InProgress),
            ]),
            None,
        ),
        (
            "status change",
            list(&[
                ("read", TodoStatus::Completed),
                ("edit", TodoStatus::Pending),
            ]),
            Some(vec![
                entry("read", PlanEntryStatus::Completed),
                entry("edit", PlanEntryStatus::Pending),
            ]),
        ),
        ("cleared", None, Some(Vec::new())),
    ];
    for (label, next, expected) in steps {
        todo.replace(next);
        let plan = match signals.observe(&tool_finished()).await {
            Some(SessionUpdate::Plan(plan)) => Some(plan.entries),
            None => None,
            Some(other) => panic!("{label}: expected a plan, got {other:?}"),
        };
        assert_eq!(plan, expected, "{label}");
    }
}

// Covers: a step reports its own request's context fill, with the configured
// window as ACP `size`, and 0 (no maximum) when neither config nor provider
// knows the window.
// Owner: ACP session signals
#[tokio::test]
async fn usage_reports_the_known_window_or_zero() {
    for (window, size) in [(Some(200_000), 200_000), (None, 0)] {
        let mut signals = SessionSignals::new(
            ContextGauge::of_session_for_tests(session().await, window),
            TodoState::default(),
            Vec::new(),
        );
        let step = RunEvent::StepStarted {
            step: 1,
            context_estimate: rho_sdk::ContextEstimate::from_estimated_tokens(1_234),
        };
        match signals.observe(&step).await {
            Some(SessionUpdate::UsageUpdate(update)) => {
                assert_eq!((update.used, update.size), (1_234, size), "{window:?}");
            }
            other => panic!("expected usage, got {other:?}"),
        }
    }
}
