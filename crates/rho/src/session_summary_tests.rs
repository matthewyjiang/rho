use std::{fs, path::PathBuf};

use pretty_assertions::assert_eq;
use rho_providers::model::{ContentBlock, Message};
use rho_sdk::{CompactionState, Revision, SessionId, SessionSnapshot};
use rho_tools::tool::ToolCall;
use tempfile::TempDir;

use super::persistence::{
    session_dir_in_root, summarize_session_file, SessionEntry, SESSION_TRANSCRIPT_FILE_NAME,
};
use super::tree::{NodeId, SessionNode, SessionNodeKind, StoredStateTransition};
use super::*;

fn write_entries(path: &std::path::Path, entries: &[SessionEntry]) {
    let contents = entries
        .iter()
        .map(|entry| serde_json::to_string(entry).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(path, contents).unwrap();
}

fn fixture_path(root: &TempDir, cwd: &TempDir, id: &str, created_at: u64) -> PathBuf {
    let dir = session_dir_in_root(root.path(), cwd.path());
    fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{created_at}_{id}.jsonl"))
}

#[test]
fn pure_legacy_replace_history_uses_display_not_model_replacement() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let id = "legacy-replace";
    let path = fixture_path(&root, &cwd, id, 100);
    write_entries(
        &path,
        &[
            SessionEntry::Session {
                version: 1,
                id: id.into(),
                timestamp: "100".into(),
                cwd: cwd.path().to_path_buf(),
                agent_id: None,
                agent_fingerprint: None,
            },
            SessionEntry::Message {
                timestamp: "101".into(),
                message: Message::user_text("model prompt"),
                display_message: Some(Box::new(Message::user_text("/goal model prompt"))),
            },
            SessionEntry::Message {
                timestamp: "102".into(),
                message: Message::assistant_text("answer"),
                display_message: None,
            },
            SessionEntry::ReplaceHistory {
                timestamp: "103".into(),
                messages: vec![Message::user_text("compacted model summary")],
            },
            SessionEntry::Message {
                timestamp: "104".into(),
                message: Message::assistant_text("after"),
                display_message: None,
            },
        ],
    );

    let summary = summarize_session_file(&path, cwd.path()).unwrap().summary;
    // Tree active display keeps pre-replacement display messages; ReplaceHistory
    // only rewrites model history.
    assert_eq!(summary.message_count, 3);
    assert_eq!(
        summary.first_user_message.as_deref(),
        Some("/goal model prompt")
    );
    assert_eq!(
        summary.last_user_message.as_deref(),
        Some("/goal model prompt")
    );
    assert_eq!(summary.updated_at, 104);
    assert_eq!(summary.created_at, 100);
}

#[test]
fn summarize_prefers_display_message_over_model_message() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let id = "display-pref";
    let path = fixture_path(&root, &cwd, id, 200);
    write_entries(
        &path,
        &[
            SessionEntry::Session {
                version: 3,
                id: id.into(),
                timestamp: "200".into(),
                cwd: cwd.path().to_path_buf(),
                agent_id: None,
                agent_fingerprint: None,
            },
            SessionEntry::Message {
                timestamp: "201".into(),
                message: Message::user_text("hidden model text"),
                display_message: Some(Box::new(Message::user_text("visible display text"))),
            },
        ],
    );

    let summary = summarize_session_file(&path, cwd.path()).unwrap().summary;
    assert_eq!(summary.message_count, 1);
    assert_eq!(
        summary.first_user_message.as_deref(),
        Some("visible display text")
    );
}

#[test]
fn tree_summary_follows_active_leaf_after_branch_switch() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let session_id = SessionId::from_string(session.id().to_owned()).unwrap();

    let root_snapshot = SessionSnapshot::new(
        session_id.clone(),
        Revision::from_u64(1),
        vec![Message::user_text("root")],
        ModelIdentity::new("provider", "api", "model"),
        CompactionState::default(),
    );
    session
        .save_snapshot(&root_snapshot, root_snapshot.history())
        .unwrap();
    let root_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();

    let left = SessionSnapshot::new(
        session_id.clone(),
        Revision::from_u64(2),
        vec![Message::user_text("root"), Message::assistant_text("left")],
        ModelIdentity::new("provider", "api", "model"),
        CompactionState::default(),
    );
    session.save_snapshot(&left, &left.history()[1..]).unwrap();

    session.set_leaf(&root_id).unwrap();
    let right = SessionSnapshot::new(
        session_id,
        Revision::from_u64(2),
        vec![
            Message::user_text("root"),
            Message::assistant_text("right branch"),
        ],
        ModelIdentity::new("provider", "api", "model"),
        CompactionState::default(),
    );
    session
        .save_snapshot(&right, &right.history()[1..])
        .unwrap();

    let record = summarize_session_file(session.path(), cwd.path()).unwrap();
    assert_eq!(record.summary.message_count, 2);
    assert_eq!(record.summary.first_user_message.as_deref(), Some("root"));
    // Active leaf is the right branch; summary must not stick to the left leaf.
    assert_eq!(record.branch_count, 1);
    assert_eq!(record.node_count, 3);
    let tree = session.session_tree().unwrap();
    assert_eq!(
        tree.active_state().unwrap().model.last(),
        Some(&Message::assistant_text("right branch"))
    );
}

#[test]
fn summarize_drops_incomplete_tool_tail_from_active_display() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let id = "tool-tail";
    let path = fixture_path(&root, &cwd, id, 300);
    write_entries(
        &path,
        &[
            SessionEntry::Session {
                version: 3,
                id: id.into(),
                timestamp: "300".into(),
                cwd: cwd.path().to_path_buf(),
                agent_id: None,
                agent_fingerprint: None,
            },
            SessionEntry::Message {
                timestamp: "301".into(),
                message: Message::user_text("please run"),
                display_message: None,
            },
            SessionEntry::Message {
                timestamp: "302".into(),
                message: Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
                    id: "call-1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({}),
                })]),
                display_message: None,
            },
            // Incomplete: tool result missing.
        ],
    );

    let summary = summarize_session_file(&path, cwd.path()).unwrap().summary;
    assert_eq!(summary.message_count, 1);
    assert_eq!(summary.first_user_message.as_deref(), Some("please run"));
    assert_eq!(summary.last_user_message.as_deref(), Some("please run"));
}

#[test]
fn v4_tree_session_summary_matches_active_display() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let dir = session_dir_in_root(root.path(), cwd.path());
    fs::create_dir_all(&dir).unwrap();
    let session_dir = dir.join("500_upgraded-tree");
    fs::create_dir_all(&session_dir).unwrap();
    let path = session_dir.join(SESSION_TRANSCRIPT_FILE_NAME);
    let session_id = "upgraded-tree";
    let node_id = NodeId::from_string("leaf-1").unwrap();
    let snapshot = SessionSnapshot::new(
        SessionId::from_string(session_id).unwrap(),
        Revision::from_u64(1),
        vec![Message::user_text("model only")],
        ModelIdentity::new("provider", "api", "model"),
        CompactionState::default(),
    );
    write_entries(
        &path,
        &[
            SessionEntry::Session {
                version: 4,
                id: session_id.into(),
                timestamp: "500".into(),
                cwd: cwd.path().to_path_buf(),
                agent_id: None,
                agent_fingerprint: None,
            },
            SessionEntry::Node {
                node: SessionNode {
                    id: node_id,
                    parent_id: None,
                    timestamp: "501".into(),
                    kind: SessionNodeKind::Commit,
                    compaction_facts: None,
                    transition: StoredStateTransition::Snapshot {
                        snapshot: Box::new(snapshot),
                    },
                    display_messages: vec![super::persistence::StoredDisplayMessage {
                        timestamp: "501".into(),
                        message: Message::user_text("tree display prompt"),
                    }],
                },
            },
        ],
    );

    let record = summarize_session_file(&path, cwd.path()).unwrap();
    assert_eq!(record.summary.message_count, 1);
    assert_eq!(
        record.summary.first_user_message.as_deref(),
        Some("tree display prompt")
    );
    assert_eq!(record.node_count, 1);
    assert_eq!(record.effective_format_version, 4);
}

// Covers: turn save must write the same index row a full transcript parse would
// Owner: session persistence
#[test]
fn save_snapshot_index_matches_full_file_summarize() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let session_id = SessionId::from_string(session.id().to_owned()).unwrap();

    let root_snapshot = SessionSnapshot::new(
        session_id.clone(),
        Revision::from_u64(1),
        vec![Message::user_text("root prompt")],
        ModelIdentity::new("provider", "api", "model"),
        CompactionState::default(),
    );
    session
        .save_snapshot(&root_snapshot, root_snapshot.history())
        .unwrap();
    assert_indexed_record_matches_file(&session, root.path());

    let root_id = session
        .session_tree()
        .unwrap()
        .active_leaf_id()
        .unwrap()
        .clone();
    let left = SessionSnapshot::new(
        session_id.clone(),
        Revision::from_u64(2),
        vec![
            Message::user_text("root prompt"),
            Message::assistant_text("left"),
        ],
        ModelIdentity::new("provider", "api", "model"),
        CompactionState::default(),
    );
    session.save_snapshot(&left, &left.history()[1..]).unwrap();
    assert_indexed_record_matches_file(&session, root.path());

    session.set_leaf(&root_id).unwrap();
    assert_indexed_record_matches_file(&session, root.path());

    let right = SessionSnapshot::new(
        session_id,
        Revision::from_u64(2),
        vec![
            Message::user_text("root prompt"),
            Message::user_text("right prompt"),
        ],
        ModelIdentity::new("provider", "api", "model"),
        CompactionState::default(),
    );
    session
        .save_snapshot(&right, &right.history()[1..])
        .unwrap();
    assert_indexed_record_matches_file(&session, root.path());
}

fn assert_indexed_record_matches_file(session: &Session, session_root: &std::path::Path) {
    let from_file = summarize_session_file(session.path(), session.cwd()).unwrap();
    let indexed = indexed_record(session_root, session.id());
    assert_eq!(indexed.summary, from_file.summary);
    assert_eq!(indexed.node_count, from_file.node_count);
    assert_eq!(indexed.branch_count, from_file.branch_count);
    assert_eq!(indexed.active_leaf_id, from_file.active_leaf_id);
    assert_eq!(
        indexed.effective_format_version,
        from_file.effective_format_version
    );
    assert_eq!(indexed.file_size, from_file.file_size);
}

fn indexed_record(session_root: &std::path::Path, id: &str) -> SessionIndexRecord {
    let connection = rusqlite::Connection::open(session_root.join("index.sqlite3")).unwrap();
    connection
        .query_row(
            "select id, path, cwd, created_at, updated_at, message_count, title,
                    first_user_message, last_user_message, file_size, file_mtime,
                    node_count, branch_count, active_leaf_id, effective_format_version
             from sessions where id = ?1",
            [id],
            |row| {
                Ok(SessionIndexRecord {
                    summary: SessionSummary {
                        id: row.get(0)?,
                        path: PathBuf::from(row.get::<_, String>(1)?),
                        cwd: PathBuf::from(row.get::<_, String>(2)?),
                        created_at: row.get::<_, i64>(3)?.max(0) as u64,
                        updated_at: row.get::<_, i64>(4)?.max(0) as u64,
                        message_count: row.get::<_, i64>(5)?.max(0) as u64,
                        title: row.get(6)?,
                        first_user_message: row.get(7)?,
                        last_user_message: row.get(8)?,
                    },
                    file_size: row.get(9)?,
                    file_mtime: row.get(10)?,
                    node_count: row.get::<_, i64>(11)?.max(0) as u64,
                    branch_count: row.get::<_, i64>(12)?.max(0) as u64,
                    active_leaf_id: row.get(13)?,
                    effective_format_version: row.get::<_, i64>(14)?.max(0) as u32,
                })
            },
        )
        .unwrap()
}
