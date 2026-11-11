use std::fs;

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

#[test]
fn summarize_prefers_display_message_over_model_message() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    session
        .append_message_with_display(
            &Message::user_text("hidden model text"),
            &Message::user_text("visible display text"),
        )
        .unwrap();

    let summary = summarize_session_file(session.path(), cwd.path())
        .unwrap()
        .summary;
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
fn summarize_counts_incomplete_active_display() {
    let root = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    session
        .append_message(&Message::user_text("please run"))
        .unwrap();
    // Incomplete: the tool result is missing.
    session
        .append_message(&Message::Assistant(vec![ContentBlock::ToolCall(
            ToolCall::new("call-1".into(), "bash".into(), serde_json::json!({})),
        )]))
        .unwrap();

    let summary = summarize_session_file(session.path(), cwd.path())
        .unwrap()
        .summary;
    assert_eq!(summary.message_count, 2);
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
                agent_role: crate::session::agent_identity::SessionAgentRole::Root,
                launch_agent: None,
                origin: None,
                parent_session: None,
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
                    display_messages: vec![super::display_rows::StoredDisplayRow::Inline {
                        timestamp: Some("501".into()),
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
