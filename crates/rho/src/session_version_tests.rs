use super::*;
use crate::model::{AssistantMessage, ProviderContextBlock};
use serde_json::json;

const SESSION_V1: &str = include_str!("session/fixtures/session-v1.jsonl");
const SESSION_V2: &str = include_str!("session/fixtures/session-v2.jsonl");
const SESSION_V3: &str = include_str!("session/fixtures/session-v3.jsonl");

#[test]
fn every_supported_application_session_fixture_restores_as_a_snapshot() {
    let cases = [
        (
            1,
            "11111111-1111-4111-8111-111111111111",
            SESSION_V1,
            4,
            1,
            1,
            "rho:migrated-v1",
            3,
        ),
        (
            2,
            "22222222-2222-4222-8222-222222222222",
            SESSION_V2,
            4,
            1,
            3,
            "rho:fixture-session",
            2,
        ),
        (
            3,
            "33333333-3333-4333-8333-333333333333",
            SESSION_V3,
            2,
            0,
            0,
            "rho:delta-fixture",
            2,
        ),
    ];

    for (version, id, fixture, revision, compactions, removed, cache_key, display_len) in cases {
        let (root, cwd, session) = session_from_fixture(id, fixture);
        let first: serde_json::Value =
            serde_json::from_str(fixture.lines().next().unwrap()).unwrap();
        assert_eq!(first["version"], version);

        let snapshot = session
            .snapshot_for_resume(
                ModelIdentity::new("target", "fixture-api", "fixture-model"),
                "rho:migrated-v1".into(),
            )
            .unwrap();
        let histories = read_histories(session.path()).unwrap();

        assert_eq!(
            snapshot.schema_version(),
            rho_sdk::SESSION_SNAPSHOT_SCHEMA_VERSION
        );
        assert_eq!(snapshot.session_id().as_str(), id);
        assert_eq!(snapshot.revision(), Revision::from_u64(revision));
        assert_eq!(snapshot.compaction().completed_compactions(), compactions);
        assert_eq!(snapshot.compaction().removed_messages(), removed);
        assert_eq!(snapshot.prompt_cache_key(), Some(cache_key));
        assert_eq!(snapshot.history(), histories.model);
        assert_eq!(histories.model.len(), 2);
        assert_eq!(histories.display.len(), display_len);

        drop((root, cwd));
    }
}

#[test]
fn v1_session_migrates_on_atomic_snapshot_save_without_losing_display_history() {
    let (_root, _cwd, session) =
        session_from_fixture("11111111-1111-4111-8111-111111111111", SESSION_V1);
    let snapshot = session
        .snapshot_for_resume(
            ModelIdentity::new("target", "api", "model"),
            "rho:migrated-v1".into(),
        )
        .unwrap();

    session.save_snapshot(&snapshot, &[]).unwrap();

    let entries = read_entries(session.path()).unwrap();
    let saved = entries.last().unwrap();
    assert!(
        matches!(saved, SessionEntry::Snapshot { snapshot: stored, display_messages, .. }
        if stored.schema_version() == rho_sdk::SESSION_SNAPSHOT_SCHEMA_VERSION
            && stored.compaction() == snapshot.compaction()
            && display_messages.is_empty())
    );
}

#[test]
fn rejects_sessions_from_unsupported_or_malformed_versions() {
    for (name, contents, expected) in [
        (
            "future-version",
            format!(
                "{{\"type\":\"session\",\"version\":{},\"id\":\"future-version\",\"timestamp\":\"1\",\"cwd\":\"/tmp\"}}\n",
                SESSION_VERSION + 1
            ),
            "unsupported session version",
        ),
        (
            "pre-version",
            "{\"type\":\"session\",\"version\":0,\"id\":\"pre-version\",\"timestamp\":\"1\",\"cwd\":\"/tmp\"}\n".into(),
            "unsupported session version",
        ),
        (
            "malformed",
            "{\"type\":\"session\",\"version\":2,\"id\":\"malformed\",\"timestamp\":\"1\",\"cwd\":\"/tmp\"}\n{\"type\":\"snapshot\",\"timestamp\":\"2\",\"snapshot\":{}}\n".into(),
            "missing field",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("1_{name}.jsonl"));
        fs::write(&path, contents).unwrap();

        let error = summarize_session_file(&path, directory.path()).unwrap_err();
        assert!(error.to_string().contains(expected), "{error:#}");
    }
}

#[test]
fn rejects_snapshot_delta_without_matching_base_revision() {
    let corrupted = SESSION_V3.replacen("\"base_revision\":1", "\"base_revision\":0", 1);
    let (_root, _cwd, session) =
        session_from_fixture("33333333-3333-4333-8333-333333333333", &corrupted);

    let error = session
        .snapshot_for_resume(
            ModelIdentity::new("target", "api", "model"),
            "rho:fallback".into(),
        )
        .unwrap_err();

    assert!(error.to_string().contains("base revision"), "{error:#}");
}

#[test]
fn consecutive_snapshot_saves_append_only_new_history() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let identity = ModelIdentity::new("provider", "api", "model");
    let id = SessionId::from_string(session.id().to_owned()).unwrap();
    let first = SessionSnapshot::new(
        id.clone(),
        Revision::from_u64(1),
        vec![Message::user_text("history only in base")],
        identity.clone(),
        CompactionState::default(),
    )
    .with_prompt_cache_key("rho:fallback");
    let second = SessionSnapshot::new(
        id,
        Revision::from_u64(2),
        vec![
            Message::user_text("history only in base"),
            Message::assistant_text("new delta message"),
        ],
        identity.clone(),
        CompactionState::default(),
    )
    .with_prompt_cache_key("rho:fallback");

    session.save_snapshot(&first, first.history()).unwrap();
    session
        .save_snapshot(&second, &second.history()[1..])
        .unwrap();

    let entries = read_entries(session.path()).unwrap();
    assert!(matches!(entries[1], SessionEntry::Snapshot { .. }));
    assert!(matches!(entries[2], SessionEntry::SnapshotDelta { .. }));
    let last_record = fs::read_to_string(session.path())
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .to_string();
    assert!(!last_record.contains("history only in base"));
    assert!(last_record.contains("new delta message"));

    let restored = session
        .snapshot_for_resume(identity, "rho:fallback".into())
        .unwrap();
    assert_eq!(restored, second);
    assert_eq!(
        read_histories(session.path()).unwrap().display,
        second.history()
    );
}

#[test]
fn history_replacement_writes_a_new_complete_snapshot_base() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let identity = ModelIdentity::new("provider", "api", "model");
    let id = SessionId::from_string(session.id().to_owned()).unwrap();
    let first = SessionSnapshot::new(
        id.clone(),
        Revision::from_u64(2),
        vec![
            Message::user_text("old"),
            Message::assistant_text("history"),
        ],
        identity.clone(),
        CompactionState::default(),
    )
    .with_prompt_cache_key("rho:fallback");
    let compacted = SessionSnapshot::new(
        id,
        Revision::from_u64(3),
        vec![Message::user_text("compact summary")],
        identity.clone(),
        CompactionState::default(),
    )
    .with_prompt_cache_key("rho:fallback");

    session.save_snapshot(&first, &[]).unwrap();
    session.save_snapshot(&compacted, &[]).unwrap();

    assert!(matches!(
        read_entries(session.path()).unwrap().last(),
        Some(SessionEntry::Snapshot { .. })
    ));
    assert_eq!(
        session
            .snapshot_for_resume(identity, "rho:fallback".into())
            .unwrap(),
        compacted
    );
}

#[test]
fn failed_snapshot_save_retains_the_previous_complete_revision() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let identity = ModelIdentity::new("provider", "api", "model");
    let first = SessionSnapshot::new(
        SessionId::from_string(session.id().to_owned()).unwrap(),
        Revision::from_u64(1),
        vec![Message::user_text("durable")],
        identity.clone(),
        CompactionState::default(),
    )
    .with_prompt_cache_key("rho:durable");
    session
        .save_snapshot(&first, &[Message::user_text("display durable")])
        .unwrap();
    let before = fs::read(session.path()).unwrap();
    let wrong_id = SessionSnapshot::new(
        SessionId::from_string("different-session").unwrap(),
        Revision::from_u64(2),
        vec![Message::user_text("must not commit")],
        identity.clone(),
        CompactionState::default(),
    );

    assert!(session.save_snapshot(&wrong_id, &[]).is_err());
    assert_eq!(fs::read(session.path()).unwrap(), before);

    OpenOptions::new()
        .append(true)
        .open(session.path())
        .unwrap()
        .write_all(b"{\"type\":\"snapshot\"")
        .unwrap();
    let restored = session
        .snapshot_for_resume(identity.clone(), "rho:fallback".into())
        .unwrap();
    assert_eq!(restored.revision(), first.revision());
    assert_eq!(restored.history(), first.history());

    session.save_snapshot(&first, &[]).unwrap();
    let restored_after_repair = session
        .snapshot_for_resume(identity, "rho:fallback".into())
        .unwrap();
    assert_eq!(restored_after_repair, first);
}

#[test]
fn resumed_snapshot_reports_and_filters_incompatible_provider_context() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let source = ModelIdentity::new("source", "source-api", "source-model");
    let target = ModelIdentity::new("target", "target-api", "target-model");
    let snapshot = SessionSnapshot::new(
        SessionId::from_string(session.id().to_owned()).unwrap(),
        Revision::from_u64(1),
        vec![Message::assistant(AssistantMessage {
            content: vec![ContentBlock::Text("portable answer".into())],
            provider_context: vec![ProviderContextBlock {
                identity: source.clone(),
                kind: "source_native_context".into(),
                position: None,
                data: json!({"opaque": true}),
            }],
            ..AssistantMessage::default()
        })],
        source,
        CompactionState::default(),
    );
    session
        .save_snapshot(&snapshot, snapshot.history())
        .unwrap();

    let resumed = session
        .snapshot_for_resume(target.clone(), "rho:resume".into())
        .unwrap();
    let report = resumed.provider_context_omissions(&target);
    let Message::EnrichedAssistant(message) = resumed.history()[0].clone() else {
        panic!("expected enriched assistant");
    };
    let prepared = rho_sdk::model::handoff::prepare_assistant(*message, &target);

    assert_eq!(report.omitted_provider_context, 1);
    assert_eq!(report.omitted_kinds, ["source_native_context"]);
    assert!(prepared.replay_context.is_empty());
}

fn session_from_fixture(
    id: &str,
    fixture: &str,
) -> (tempfile::TempDir, tempfile::TempDir, Session) {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let dir = session_dir_in_root(root.path(), cwd.path());
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("100_{id}.jsonl"));
    fs::write(&path, fixture).unwrap();
    let session = Session::from_parts(root.path(), cwd.path(), id.into(), path);
    (root, cwd, session)
}
