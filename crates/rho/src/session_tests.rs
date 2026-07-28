use std::ops::Deref;

use tempfile::TempDir;

use super::tree::{SessionNodeKind, StoredStateTransition};
use super::*;
use {
    rho_providers::model::{AssistantMessage, ContentBlock, ModelIdentity, ProviderContextBlock},
    rho_tools::tool::ToolCall,
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
fn enriched_assistant_context_round_trips_for_resume() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    let identity = ModelIdentity::new("openai-codex", "openai-responses", "gpt-test");
    session
        .append_message(&Message::assistant(AssistantMessage {
            content: vec![ContentBlock::Text("answer".into())],
            provenance: Some(identity.clone()),
            reasoning_summary: Some("verified the result".into()),
            provider_context: vec![ProviderContextBlock {
                identity,
                kind: "openai_response_output_item".into(),
                position: None,
                data: serde_json::json!({"type": "reasoning", "encrypted_content": "signed"}),
            }],
        }))
        .unwrap();

    let (_, messages) = Session::open_by_id_in_root(&root, &cwd, session.id()).unwrap();

    assert!(matches!(
        &messages[0],
        Message::EnrichedAssistant(message)
            if message.reasoning_summary.as_deref() == Some("verified the result")
                && message.provenance.as_ref().is_some_and(|value| value.model == "gpt-test")
                && message.provider_context.len() == 1
    ));
}

#[test]
fn resumes_session_by_id_from_a_different_workspace() {
    let root = temp_session_root();
    let created_cwd = temp_cwd();
    let session = Session::create_in_root(&root, &created_cwd).unwrap();
    session
        .append_message(&Message::assistant_text("from the original workspace"))
        .unwrap();
    let id = session.id().to_string();

    let other_cwd = temp_cwd();
    let (resumed, messages) = Session::open_by_id_in_root(&root, &other_cwd, &id).unwrap();

    assert_eq!(resumed.id(), id);
    assert_eq!(
        resumed.cwd(),
        &*created_cwd,
        "resume adopts the session's original workspace, not the current directory"
    );
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        &messages[0],
        Message::Assistant(content)
            if matches!(content.as_slice(), [ContentBlock::Text(text)] if text == "from the original workspace")
    ));
}

#[test]
fn resume_by_id_errors_when_the_original_workspace_is_gone() {
    let root = temp_session_root();
    let parent = temp_cwd();
    let original = parent.join("project");
    std::fs::create_dir(&original).unwrap();
    let session = Session::create_in_root(&root, &original).unwrap();
    session
        .append_message(&Message::assistant_text("work in progress"))
        .unwrap();
    let id = session.id().to_string();

    std::fs::remove_dir_all(&original).unwrap();

    let other_cwd = temp_cwd();
    let error = Session::open_by_id_in_root(&root, &other_cwd, &id).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("no longer an accessible directory"),
        "expected a workspace-gone recovery error, got: {error}"
    );
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
        matches!(entry, SessionEntry::Node { node }
            if matches!(&node.transition, StoredStateTransition::Snapshot { snapshot }
                if snapshot.history().iter().any(|message| message == &Message::user_text("old user"))))
    }));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        SessionEntry::Node { node } if node.kind == SessionNodeKind::Compaction
    )));

    let (_session, histories) =
        Session::open_by_id_with_histories_in_root(&root, &cwd, session.id()).unwrap();

    assert_eq!(histories.model.len(), 3);
    assert!(
        matches!(&histories.model[0], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "summary"))
    );
    assert!(
        matches!(&histories.model[2], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "after replacement"))
    );
    assert_eq!(histories.display.len(), 4);
    assert!(
        matches!(&histories.display[0], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "old user"))
    );
    assert!(
        matches!(&histories.display[1], Message::Assistant(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "old assistant"))
    );
    assert!(matches!(&histories.display[2], Message::Assistant(blocks)
            if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text.contains("Compacted context"))));
    assert!(
        matches!(&histories.display[3], Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "after replacement"))
    );
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
fn workspace_key_avoids_separator_collisions() {
    let slash_path = PathBuf::from("/tmp/rho-workspace/a/b");
    let dash_path = PathBuf::from("/tmp/rho-workspace/a-b");

    assert_eq!(encode_cwd(&slash_path), encode_cwd(&dash_path));
    assert_ne!(workspace_key(&slash_path), workspace_key(&dash_path));
}

#[test]
fn drops_incomplete_tool_call_tail_on_load() {
    let plain = Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: "call-1".into(),
        name: "bash".into(),
        arguments: serde_json::json!({"command": "echo hi"}),
    })]);
    let enriched = Message::assistant(AssistantMessage {
        content: vec![ContentBlock::ToolCall(ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "echo hi"}),
        })],
        provenance: Some(ModelIdentity::new(
            "openai-codex",
            "openai-responses",
            "gpt-test",
        )),
        reasoning_summary: None,
        provider_context: Vec::new(),
    });

    for assistant in [plain, enriched] {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session
            .append_message(&Message::user_text("run a tool"))
            .unwrap();
        session.append_message(&assistant).unwrap();

        let (_, messages) = Session::open_by_id_in_root(&root, &cwd, session.id()).unwrap();

        assert_eq!(messages.len(), 1);
        assert!(matches!(&messages[0], Message::User(_)));
    }
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
fn append_repairs_external_writes_despite_cached_cursor() {
    for torn in [true, false] {
        let root = temp_session_root();
        let cwd = temp_cwd();
        let session = Session::create_in_root(&root, &cwd).unwrap();
        session
            .append_message(&Message::user_text("first"))
            .unwrap();
        session
            .append_message(&Message::user_text("second"))
            .unwrap();
        let tail = if torn {
            b"{\"type\":\"set_leaf\"".to_vec()
        } else {
            let active_leaf_id = session
                .session_tree()
                .unwrap()
                .active_leaf_id()
                .unwrap()
                .clone();
            serde_json::to_vec(&SessionEntry::SetLeaf {
                timestamp: "1".into(),
                target_id: active_leaf_id,
            })
            .unwrap()
        };
        OpenOptions::new()
            .append(true)
            .open(session.path())
            .unwrap()
            .write_all(&tail)
            .unwrap();

        session
            .append_message(&Message::user_text("third"))
            .unwrap();

        let (_, messages) = Session::open_by_id_in_root(&root, &cwd, session.id()).unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages.last(), Some(&Message::user_text("third")));
    }
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
fn list_removes_stale_index_rows() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    assert_eq!(Session::list_in_root(&root, &cwd).unwrap().len(), 1);
    remove_session_storage(session.path());

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
    let workspace_mode = fs::metadata(session.path().parent().unwrap().parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let session_dir_mode = fs::metadata(session.path().parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let file_mode = fs::metadata(session.path()).unwrap().permissions().mode() & 0o777;

    assert_eq!(root_mode, 0o700);
    assert_eq!(workspace_mode, 0o700);
    assert_eq!(session_dir_mode, 0o700);
    assert_eq!(file_mode, 0o600);
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

    assert_eq!(export.messages.len(), 2);
    assert!(
        matches!(&export.messages[0].message, Message::User(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "original"))
    );
    assert!(
        matches!(&export.messages[1].message, Message::Assistant(blocks)
            if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text.contains("Compacted context")))
    );
}

fn temp_session_root() -> TestDir {
    TestDir(tempfile::tempdir().unwrap())
}

fn temp_cwd() -> TestDir {
    TestDir(tempfile::tempdir().unwrap())
}

fn remove_session_storage(path: &Path) {
    if path.file_name().and_then(|name| name.to_str()) == Some("session.jsonl") {
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    } else {
        fs::remove_file(path).unwrap();
    }
}

fn write_minimal_session_file(root: &Path, cwd: &Path, id: &str) {
    write_session_file(root, cwd, id, 0, &[]);
}

fn write_session_file(root: &Path, cwd: &Path, id: &str, timestamp: u64, prompts: &[&str]) {
    let dir = session_dir_in_root(root, cwd);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{timestamp}_{id}.jsonl"));
    let mut entries = vec![SessionEntry::Session {
        version: 3,
        id: id.into(),
        timestamp: timestamp.to_string(),
        cwd: cwd.to_path_buf(),
        agent_id: None,
        agent_fingerprint: None,
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

#[test]
fn opens_legacy_flat_jsonl_sessions_by_id() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    write_session_file(&root, &cwd, "flat-legacy-id", 42, &["hello legacy"]);

    let (session, messages) = Session::open_by_id_in_root(&root, &cwd, "flat-legacy-id").unwrap();

    assert_eq!(session.id(), "flat-legacy-id");
    assert!(session.path().extension().and_then(|ext| ext.to_str()) == Some("jsonl"));
    assert!(session
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap()
        .ends_with("_flat-legacy-id.jsonl"));
    assert!(matches!(
        messages.as_slice(),
        [Message::User(blocks)] if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "hello legacy")
    ));
}

#[test]
fn session_web_dir_uses_folder_sidecar_and_legacy_companion() {
    let folder = PathBuf::from("/tmp/ws/100_abc/session.jsonl");
    assert_eq!(
        super::persistence::session_web_dir(&folder),
        Some(PathBuf::from("/tmp/ws/100_abc/web"))
    );

    let legacy = PathBuf::from("/tmp/ws/100_abc.jsonl");
    assert_eq!(
        super::persistence::session_web_dir(&legacy),
        Some(PathBuf::from("/tmp/ws/100_abc.web"))
    );
}

#[test]
fn deletes_folder_session_and_cascades_parent_linked_runs() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let subagents = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session
        .append_message(&Message::user_text("delete me"))
        .unwrap();
    let id = session.id().to_string();
    let session_dir = session.path().parent().unwrap().to_path_buf();
    let web_dir = session_dir.join("web");
    fs::create_dir_all(&web_dir).unwrap();
    fs::write(web_dir.join("blob.bin"), b"sidecar").unwrap();

    let linked = super::delete::write_linked_run_for_tests(
        subagents.path(),
        "aa11bb",
        &id,
        crate::subagent::RunState::Ok,
    );
    let other = super::delete::write_linked_run_for_tests(
        subagents.path(),
        "cc22dd",
        "some-other-session",
        crate::subagent::RunState::Ok,
    );

    let outcome = Session::delete_by_id_in_roots(
        &root,
        subagents.path(),
        &cwd,
        &id,
        DeleteOptions::default(),
    )
    .unwrap();

    assert_eq!(outcome.id, id);
    assert_eq!(outcome.deleted_run_count, 1);
    assert!(!session_dir.exists());
    assert!(!linked.exists());
    assert!(other.exists());
    assert!(Session::list_in_root_for_test(&root, &cwd)
        .unwrap()
        .is_empty());

    let err = Session::delete_by_id_in_roots(
        &root,
        subagents.path(),
        &cwd,
        &id,
        DeleteOptions::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("no session found"), "{err}");
}

#[test]
fn deletes_nested_runs_with_folder_session() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let subagents = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    let nested = super::delete::write_linked_run_for_tests(
        &session.subagents_dir().unwrap(),
        "ab12cd",
        session.id(),
        crate::subagent::RunState::Ok,
    );

    let outcome = Session::delete_by_id_in_roots(
        &root,
        subagents.path(),
        &cwd,
        session.id(),
        DeleteOptions::default(),
    )
    .unwrap();

    assert_eq!(outcome.deleted_run_count, 1);
    assert!(!nested.exists());
}

#[test]
fn refuses_live_nested_run_without_force() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let subagents = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    let nested = super::delete::write_linked_run_for_tests(
        &session.subagents_dir().unwrap(),
        "ab12cd",
        session.id(),
        crate::subagent::RunState::Running,
    );

    let error = Session::delete_by_id_in_roots(
        &root,
        subagents.path(),
        &cwd,
        session.id(),
        DeleteOptions::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("still running"), "{error}");

    let outcome = Session::delete_by_id_in_roots(
        &root,
        subagents.path(),
        &cwd,
        session.id(),
        DeleteOptions {
            force: true,
            protect_session_id: None,
        },
    )
    .unwrap();
    assert_eq!(outcome.forced_run_ids, vec!["ab12cd".to_string()]);
    assert_eq!(outcome.deleted_run_count, 1);
    assert!(!nested.exists());
}

#[test]
fn deletes_legacy_flat_session_and_web_companion() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    write_session_file(&root, &cwd, "legacy-delete-id", 99, &["legacy delete"]);
    let dir = session_dir_in_root(&root, &cwd);
    let transcript = dir.join("99_legacy-delete-id.jsonl");
    let web = dir.join("99_legacy-delete-id.web");
    fs::create_dir_all(&web).unwrap();
    fs::write(web.join("page.html"), b"<html></html>").unwrap();
    // Ensure the index knows about the legacy file before delete.
    let _ = Session::list_in_root_for_test(&root, &cwd).unwrap();

    Session::delete_by_id_in_roots(
        &root,
        tempfile::tempdir().unwrap().path(),
        &cwd,
        "legacy-delete-id",
        DeleteOptions::default(),
    )
    .unwrap();

    assert!(!transcript.exists());
    assert!(!web.exists());
}

#[test]
fn refuses_current_session_and_live_runs_without_force() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let subagents = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    let id = session.id().to_string();

    let protected = Session::delete_by_id_in_roots(
        &root,
        subagents.path(),
        &cwd,
        &id,
        DeleteOptions {
            force: false,
            protect_session_id: Some(id.clone()),
        },
    )
    .unwrap_err();
    assert!(
        protected.to_string().contains("current session"),
        "{protected}"
    );

    super::delete::write_linked_run_for_tests(
        subagents.path(),
        "ee33ff",
        &id,
        crate::subagent::RunState::Running,
    );
    let live = Session::delete_by_id_in_roots(
        &root,
        subagents.path(),
        &cwd,
        &id,
        DeleteOptions::default(),
    )
    .unwrap_err();
    assert!(live.to_string().contains("still running"), "{live}");

    let forced = Session::delete_by_id_in_roots(
        &root,
        subagents.path(),
        &cwd,
        &id,
        DeleteOptions {
            force: true,
            protect_session_id: None,
        },
    )
    .unwrap();
    assert_eq!(forced.forced_run_ids, vec!["ee33ff".to_string()]);
    assert_eq!(forced.deleted_run_count, 1);
}

#[test]
fn list_all_includes_other_workspaces() {
    let root = temp_session_root();
    let cwd_a = temp_cwd();
    let cwd_b = temp_cwd();
    let session_a = Session::create_in_root(&root, &cwd_a).unwrap();
    session_a
        .append_message(&Message::user_text("project a"))
        .unwrap();
    let session_b = Session::create_in_root(&root, &cwd_b).unwrap();
    session_b
        .append_message(&Message::user_text("project b"))
        .unwrap();

    let all = Session::list_all_in_root(&root).unwrap();
    let ids = all
        .iter()
        .map(|summary| summary.id.as_str())
        .collect::<Vec<_>>();
    assert!(ids.contains(&session_a.id()));
    assert!(ids.contains(&session_b.id()));
    assert!(all.iter().any(|summary| summary.cwd == *cwd_a));
    assert!(all.iter().any(|summary| summary.cwd == *cwd_b));
}

#[test]
fn index_self_heals_when_folder_is_gone_but_row_remains() {
    let root = temp_session_root();
    let cwd = temp_cwd();
    let session = Session::create_in_root(&root, &cwd).unwrap();
    session
        .append_message(&Message::user_text("orphaned index row"))
        .unwrap();
    let path = session.path().to_path_buf();
    remove_session_storage(&path);

    let listed = Session::list_in_root_for_test(&root, &cwd).unwrap();
    assert!(listed.is_empty());
}
