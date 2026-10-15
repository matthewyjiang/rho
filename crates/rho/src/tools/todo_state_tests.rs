use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ModelIdentity, ToolCall, ToolResult},
    CompactionState, InMemorySessionStore, Revision, SessionId,
};
use serde_json::json;

use super::*;

fn list(content: &str) -> TodoList {
    TodoList::parse(json!({"todos": [{"content": content, "status": "pending"}]})).unwrap()
}

fn snapshot(history: Vec<Message>) -> SessionSnapshot {
    SessionSnapshot::new(
        SessionId::new(),
        Revision::default(),
        history,
        ModelIdentity::new("test", "test", "test"),
        CompactionState::default(),
    )
}

// Covers: legacy replay must ignore unsuccessful and unmatched proposals, while
// snapshot metadata (including null and clear) outranks historical tool calls.
// Owner: host checklist restore compatibility.
#[test]
fn restore_uses_successful_paired_native_calls_only_without_metadata() {
    let first = list("original");
    let second = list("replacement");
    let proposal = |id: &str, list: &TodoList| {
        Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: id.into(),
            name: "todo".into(),
            arguments: serde_json::to_value(list).unwrap(),
        })])
    };
    let result = |id: &str, ok| {
        Message::ToolResult(ToolResult {
            id: id.into(),
            ok,
            content: "receipt".into(),
        })
    };
    let history = vec![
        proposal("one", &first),
        result("one", true),
        proposal("two", &second),
    ];
    for (tail, expected) in [
        (Vec::new(), Some(first.clone())),
        (vec![result("two", false)], Some(first.clone())),
        (vec![result("unmatched", true)], Some(first.clone())),
        (vec![result("two", true)], Some(second)),
    ] {
        let state = TodoState::default();
        state.restore(&snapshot([history.clone(), tail].concat()));
        assert_eq!(state.list(), expected);
    }
    for replacement in [None, Some(TodoList { todos: Vec::new() })] {
        let writer = TodoState::default();
        writer.replace(replacement.clone());
        let reader = TodoState::default();
        reader.restore(&writer.decorate(snapshot(history.clone())));
        assert_eq!(reader.list(), replacement);
    }
}

fn todo_call(id: &str, list: &TodoList) -> [Message; 2] {
    [
        Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: id.into(),
            name: "todo".into(),
            arguments: serde_json::to_value(list).unwrap(),
        })]),
        Message::ToolResult(ToolResult {
            id: id.into(),
            ok: true,
            content: "receipt".into(),
        }),
    ]
}

fn checkpoint(list: Option<TodoList>, restate: bool) -> TodoCheckpoint {
    TodoCheckpoint { list, restate }
}

// Covers: the checklist reappears once, last, only when a real replacement no
// longer ends its todo calls with it; unchanged output stays byte-identical.
// Owner: host checklist retention across compaction.
#[test]
fn retain_restates_the_list_only_when_the_replacement_lost_it() {
    let current = list("current");
    let older = list("older");
    let restated = |list: &TodoList| restatement(list).unwrap();
    let summary = Message::compaction_summary(rho_sdk::CompactionTrigger::Manual, "summary");
    let latest = Message::user_text("latest instruction");
    let compacted = [
        vec![Message::user_text("start")],
        todo_call("one", &current).to_vec(),
        vec![latest.clone()],
    ]
    .concat();
    let cases = [
        (
            "summary dropped the call",
            checkpoint(Some(current.clone()), true),
            vec![summary.clone(), latest.clone()],
            vec![summary.clone(), latest.clone(), restated(&current)],
        ),
        (
            "replacement still ends with the same call",
            checkpoint(Some(current.clone()), true),
            [vec![summary.clone()], todo_call("one", &current).to_vec()].concat(),
            [vec![summary.clone()], todo_call("one", &current).to_vec()].concat(),
        ),
        (
            "latest native call is older than a nested update",
            checkpoint(Some(current.clone()), true),
            [vec![summary.clone()], todo_call("one", &older).to_vec()].concat(),
            [
                vec![summary.clone()],
                todo_call("one", &older).to_vec(),
                vec![restated(&current)],
            ]
            .concat(),
        ),
        (
            "earlier restatement moves to the end",
            checkpoint(Some(current.clone()), true),
            vec![summary.clone(), restated(&older), latest.clone()],
            vec![summary.clone(), latest.clone(), restated(&current)],
        ),
        (
            "no room for the list",
            checkpoint(Some(current.clone()), false),
            vec![summary.clone(), restated(&older)],
            vec![summary.clone()],
        ),
        (
            "cleared list",
            checkpoint(Some(TodoList { todos: Vec::new() }), true),
            vec![summary.clone(), restated(&older)],
            vec![summary.clone()],
        ),
        (
            "unchanged output",
            checkpoint(Some(older.clone()), true),
            compacted.clone(),
            compacted.clone(),
        ),
    ];
    for (name, checkpoint, output, expected) in cases {
        let output = checkpoint
            .retain(&compacted, CompactionOutput::new(output).unwrap())
            .unwrap();
        assert_eq!(output.messages(), expected, "{name}");
    }
}

// Covers: compaction commits the list captured before it awaited the provider,
// not a newer live update that raced with it.
// Owner: host checklist metadata through SDK compaction commit.
#[tokio::test]
async fn compaction_commits_the_checklist_captured_before_it_ran() {
    use rho_sdk::{provider::ScriptedProvider, Rho, ScriptedCompactor, SessionOptions};
    let state = TodoState::default();
    let old = list("compacted checklist");
    state.replace(Some(old.clone()));
    let history = vec![
        Message::user_text("earlier instruction"),
        Message::assistant_text("earlier answer"),
    ];
    let output = state
        .checkpoint()
        .retain(
            &history,
            CompactionOutput::new(vec![Message::assistant_text("summary")]).unwrap(),
        )
        .unwrap();
    state.replace(Some(list("newer live checklist")));
    let session = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "test"),
            [],
        ))
        .compactor(ScriptedCompactor::new([output]))
        .build()
        .unwrap()
        .session(SessionOptions::new().history(history))
        .await
        .unwrap();
    session.compact().await.unwrap();
    let restored = TodoState::default();
    restored.restore(&session.snapshot());
    assert_eq!(restored.list(), Some(old));
}

// Covers: headless SDK checkpoint saves retain exact nested state even though
// no native todo call appears in history, and explicit clear remains durable.
// Owner: host headless checkpoint store adapter.
#[tokio::test]
async fn headless_checkpoint_store_persists_latest_exact_state() {
    let state = TodoState::default();
    let inner = Arc::new(InMemorySessionStore::new());
    let store = state.checkpoint_store(inner);
    let snapshot = snapshot(vec![Message::user_text("no native todo arguments")]);
    for replacement in [
        Some(list("nested exact checklist\n  indentation  ")),
        Some(TodoList { todos: Vec::new() }),
        None,
    ] {
        state.replace(replacement.clone());
        store.save(snapshot.clone()).await.unwrap();
        let saved = store.load(snapshot.session_id()).await.unwrap().unwrap();
        let reader = TodoState::default();
        reader.restore(&saved);
        assert_eq!(reader.list(), replacement);
    }
}
