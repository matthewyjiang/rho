use std::ops::Deref;

use tempfile::TempDir;

use super::*;
use crate::{
    model::{ContentBlock, ImageContent},
    tool::{ToolCall, ToolResult},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

struct TestDir(TempDir);

impl Deref for TestDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.0.path()
    }
}

impl AsRef<Path> for TestDir {
    fn as_ref(&self) -> &Path {
        self.0.path()
    }
}

#[test]
fn persists_and_loads_messages() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session
        .append_message(&Message::User(vec![
            ContentBlock::Text("hello".into()),
            ContentBlock::Image(ImageContent {
                data: "aW1n".into(),
                mime_type: "image/png".into(),
            }),
        ]))
        .unwrap();
    session
        .append_message(&Message::assistant_text("hi"))
        .unwrap();

    let (_session, messages) = Session::open_by_id_in_root(&root, &cwd, session.id()).unwrap();
    assert_eq!(messages.len(), 2);
    assert!(matches!(&messages[0], Message::User(blocks) if matches!(
        blocks.as_slice(),
        [ContentBlock::Text(text), ContentBlock::Image(image)]
            if text == "hello" && image.mime_type == "image/png" && image.data == "aW1n"
    )));
    assert!(matches!(&messages[1], Message::Assistant(_)));
}

#[test]
fn separate_display_message_round_trips_for_resume_and_export() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    let model_message = Message::user_text("internal goal-setting instructions");
    let display_message = Message::user_text("/goal all tests pass");
    session
        .append_message_with_display(&model_message, &display_message)
        .unwrap();
    session
        .append_message(&Message::assistant_text("working on it"))
        .unwrap();

    let (_, histories) =
        Session::open_by_id_with_histories_in_root(&root, &cwd, session.id()).unwrap();
    assert!(matches!(
        &histories.model[0],
        Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "internal goal-setting instructions")
    ));
    assert!(matches!(
        &histories.display[0],
        Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "/goal all tests pass")
    ));

    let export = Session::export_by_id_in_root(&root, &cwd, session.id()).unwrap();
    assert!(matches!(
        &export.messages[0].message,
        Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "/goal all tests pass")
    ));
}

#[test]
fn replace_history_round_trips_compacted_messages() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session.append_message(&Message::user_text("old")).unwrap();
    session
        .replace_history(&[
            Message::user_text("summary"),
            Message::assistant_text("recent answer"),
        ])
        .unwrap();

    let (_session, messages) = Session::open_by_id_in_root(&root, &cwd, session.id()).unwrap();

    assert_eq!(messages.len(), 2);
    assert!(
        matches!(&messages[0], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "summary"))
    );
    assert!(
        matches!(&messages[1], Message::Assistant(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "recent answer"))
    );
}

#[test]
fn replace_history_is_append_only_but_model_replay_uses_latest_replacement() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session
        .append_message(&Message::user_text("old user"))
        .unwrap();
    session
        .append_message(&Message::assistant_text("old assistant"))
        .unwrap();
    session
        .replace_history(&[
            Message::user_text("summary"),
            Message::assistant_text("recent answer"),
        ])
        .unwrap();
    session
        .append_message(&Message::user_text("after replacement"))
        .unwrap();

    let entries = read_entries(session.path()).unwrap();
    assert!(entries.iter().any(|entry| {
        matches!(entry, SessionEntry::Message { message, .. }
            if matches!(message, Message::User(blocks)
                if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "old user")))
    }));
    assert!(entries
        .iter()
        .any(|entry| matches!(entry, SessionEntry::ReplaceHistory { .. })));

    let (_session, histories) =
        Session::open_by_id_with_histories_in_root(&root, &cwd, session.id()).unwrap();

    assert_eq!(histories.model.len(), 3);
    assert!(
        matches!(&histories.model[0], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "summary"))
    );
    assert!(
        matches!(&histories.model[2], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "after replacement"))
    );
    assert_eq!(histories.display.len(), 3);
    assert!(
        matches!(&histories.display[0], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "old user"))
    );
    assert!(
        matches!(&histories.display[1], Message::Assistant(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "old assistant"))
    );
    assert!(
        matches!(&histories.display[2], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "after replacement"))
    );
}

#[test]
fn replace_history_updates_session_summary() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session.append_message(&Message::user_text("old")).unwrap();
    session
        .replace_history(&[Message::user_text("summary"), Message::user_text("latest")])
        .unwrap();

    let summaries = Session::list_in_root(&root, &cwd).unwrap();

    assert_eq!(summaries[0].message_count, 2);
    assert_eq!(summaries[0].first_user_message.as_deref(), Some("summary"));
    assert_eq!(summaries[0].last_user_message.as_deref(), Some("latest"));
}

#[test]
fn opens_session_by_uuid_prefix() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session
        .append_message(&Message::user_text("prefix match"))
        .unwrap();

    let prefix = &session.id()[..8];
    let (opened, messages) = Session::open_by_id_in_root(&root, &cwd, prefix).unwrap();

    assert_eq!(opened.id(), session.id());
    assert_eq!(messages.len(), 1);
}

#[test]
fn errors_when_uuid_prefix_is_ambiguous() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    write_minimal_session_file(&root, &cwd, "aaaaaaaa-1111-4111-8111-111111111111");
    write_minimal_session_file(&root, &cwd, "aaaaaaaa-2222-4222-8222-222222222222");

    let err = Session::open_by_id_in_root(&root, &cwd, "aaaaaaaa").unwrap_err();

    assert!(err.to_string().contains("multiple sessions match"));
}

#[test]
fn errors_when_uuid_prefix_is_missing() {
    let root = temp_session_root();
    let cwd = temp_cwd();

    let err = Session::open_by_id_in_root(&root, &cwd, "missing").unwrap_err();

    assert!(err.to_string().contains("no session found"));
}

#[test]
fn stores_sessions_under_session_root_workspace_key() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    let expected_parent = root.join(workspace_key(&cwd));

    assert_eq!(session.path().parent(), Some(expected_parent.as_path()));
}

#[test]
fn workspace_key_avoids_separator_collisions() {
    let slash_path = PathBuf::from("/tmp/rho-workspace/a/b");
    let dash_path = PathBuf::from("/tmp/rho-workspace/a-b");

    assert_eq!(encode_cwd(&slash_path), encode_cwd(&dash_path));
    assert_ne!(workspace_key(&slash_path), workspace_key(&dash_path));
}

#[test]
fn drops_incomplete_tool_call_tail_on_load() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session
        .append_message(&Message::user_text("run a tool"))
        .unwrap();
    session
        .append_message(&Message::Assistant(vec![ContentBlock::ToolCall(
            ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "echo hi"}),
            },
        )]))
        .unwrap();

    let (_session, messages) = Session::open_by_id_in_root(&root, &cwd, session.id()).unwrap();

    assert_eq!(messages.len(), 1);
    assert!(matches!(&messages[0], Message::User(_)));
}

#[test]
fn tolerates_only_truncated_final_json() {
    for (tail, should_load) in [
        (b"{\"type\":\"message\"".as_slice(), true),
        (b"{not json}\n".as_slice(), false),
        (b"{not json}".as_slice(), false),
    ] {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session
            .append_message(&Message::user_text("complete"))
            .unwrap();
        OpenOptions::new()
            .append(true)
            .open(session.path())
            .unwrap()
            .write_all(tail)
            .unwrap();

        assert_eq!(
            Session::open_by_id_in_root(&root, &cwd, session.id()).is_ok(),
            should_load
        );
    }
}

#[test]
fn keeps_complete_tool_call_turn_on_load() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session
        .append_message(&Message::Assistant(vec![ContentBlock::ToolCall(
            ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "echo hi"}),
            },
        )]))
        .unwrap();
    session
        .append_message(&Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: "hi".into(),
        }))
        .unwrap();

    let (_session, messages) = Session::open_by_id_in_root(&root, &cwd, session.id()).unwrap();

    assert_eq!(messages.len(), 2);
    assert!(matches!(&messages[0], Message::Assistant(_)));
    assert!(matches!(&messages[1], Message::ToolResult(_)));
}

#[test]
fn list_backfills_existing_sessions_and_sorts_newest_first() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let older_id = "11111111-1111-4111-8111-111111111111";
    let newer_id = "22222222-2222-4222-8222-222222222222";
    write_session_file(&root, &cwd, older_id, 10, &["older prompt"]);
    write_session_file(&root, &cwd, newer_id, 20, &["newer prompt"]);

    let summaries = Session::list_in_root(&root, &cwd).unwrap();

    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].id, newer_id);
    assert_eq!(summaries[0].message_count, 1);
    assert_eq!(
        summaries[0].first_user_message.as_deref(),
        Some("newer prompt")
    );
    assert_eq!(
        summaries[0].last_user_message.as_deref(),
        Some("newer prompt")
    );
    assert_eq!(summaries[1].id, older_id);
    assert!(root.join("index.sqlite3").exists());
}

#[test]
fn append_message_updates_session_summary() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session
        .append_message(&Message::user_text("remember this"))
        .unwrap();
    session
        .append_message(&Message::assistant_text("remembered"))
        .unwrap();

    let summaries = Session::list_in_root(&root, &cwd).unwrap();

    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, session.id());
    assert_eq!(summaries[0].message_count, 2);
    assert_eq!(
        summaries[0].first_user_message.as_deref(),
        Some("remember this")
    );
    assert_eq!(
        summaries[0].last_user_message.as_deref(),
        Some("remember this")
    );
    assert!(summaries[0].updated_at >= summaries[0].created_at);
}

#[test]
fn set_title_updates_session_summary() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session
        .append_message(&Message::user_text("write tests"))
        .unwrap();

    Session::set_title_in_root(&root, &cwd, session.id(), "Testing plan").unwrap();
    let summaries = Session::list_in_root(&root, &cwd).unwrap();

    assert_eq!(summaries[0].title.as_deref(), Some("Testing plan"));
    assert_eq!(
        summaries[0].first_user_message.as_deref(),
        Some("write tests")
    );
}

#[test]
fn list_removes_stale_index_rows() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    assert_eq!(Session::list_in_root(&root, &cwd).unwrap().len(), 1);
    fs::remove_file(session.path()).unwrap();

    let summaries = Session::list_in_root(&root, &cwd).unwrap();

    assert!(summaries.is_empty());
}

#[cfg(unix)]
#[test]
fn creates_session_paths_with_private_permissions() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();

    let root_mode = fs::metadata(&root).unwrap().permissions().mode() & 0o777;
    let dir_mode = fs::metadata(session.path().parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let file_mode = fs::metadata(session.path()).unwrap().permissions().mode() & 0o777;

    assert_eq!(root_mode, 0o700);
    assert_eq!(dir_mode, 0o700);
    assert_eq!(file_mode, 0o600);
}

#[test]
fn export_by_id_returns_metadata_and_timestamped_display_messages() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session
        .append_message(&Message::user_text("first prompt"))
        .unwrap();
    session
        .append_message(&Message::assistant_text("first answer"))
        .unwrap();
    Session::set_title_in_root(&root, &cwd, session.id(), "Export me").unwrap();

    let export = Session::export_by_id_in_root(&root, &cwd, session.id()).unwrap();

    assert_eq!(export.id, session.id());
    assert_eq!(export.cwd, cwd.to_path_buf());
    assert_eq!(export.title.as_deref(), Some("Export me"));
    assert!(export.created_at > 0);
    assert!(export.updated_at >= export.created_at);
    assert_eq!(export.messages.len(), 2);
    assert!(export
        .messages
        .iter()
        .all(|entry| entry.timestamp.is_some()));
    assert!(
        matches!(&export.messages[0].message, Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "first prompt"))
    );
    assert!(matches!(&export.messages[1].message, Message::Assistant(_)));
}

#[test]
fn export_by_id_uses_display_history_and_drops_incomplete_tool_tail() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session
        .append_message(&Message::user_text("original"))
        .unwrap();
    session
        .replace_history(&[Message::user_text("compacted summary")])
        .unwrap();
    session
        .append_message(&Message::Assistant(vec![ContentBlock::ToolCall(
            ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "echo hi"}),
            },
        )]))
        .unwrap();

    let export = Session::export_by_id_in_root(&root, &cwd, session.id()).unwrap();

    assert_eq!(export.messages.len(), 1);
    assert!(
        matches!(&export.messages[0].message, Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "original"))
    );
}

fn temp_session_root() -> TestDir {
    TestDir(tempfile::tempdir().unwrap())
}

fn temp_cwd() -> TestDir {
    TestDir(tempfile::tempdir().unwrap())
}

fn write_minimal_session_file(root: &Path, cwd: &Path, id: &str) {
    write_session_file(root, cwd, id, 0, &[]);
}

fn write_session_file(root: &Path, cwd: &Path, id: &str, timestamp: u64, prompts: &[&str]) {
    let dir = session_dir_in_root(root, cwd);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{timestamp}_{id}.jsonl"));
    let mut entries = vec![SessionEntry::Session {
        version: SESSION_VERSION,
        id: id.into(),
        timestamp: timestamp.to_string(),
        cwd: cwd.to_path_buf(),
    }];
    entries.extend(prompts.iter().map(|prompt| SessionEntry::Message {
        timestamp: timestamp.to_string(),
        message: Message::user_text(*prompt),
        display_message: None,
    }));
    let contents = entries
        .into_iter()
        .map(|entry| serde_json::to_string(&entry).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(path, contents).unwrap();
}
