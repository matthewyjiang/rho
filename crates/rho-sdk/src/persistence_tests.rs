use std::str::FromStr;

use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    model::{AbortedAssistant, ContentBlock, Message, ModelIdentity, ProviderContextBlock},
    InMemorySessionStore, Revision, SessionId,
};

use super::{
    SessionSnapshot, SessionStore, MIN_SESSION_SNAPSHOT_SCHEMA_VERSION,
    SESSION_SNAPSHOT_SCHEMA_VERSION,
};

const SNAPSHOT_V1: &str = include_str!("fixtures/session-snapshot-v1.json");
const SNAPSHOT_V2: &str = include_str!("fixtures/session-snapshot-v2.json");

fn snapshot() -> SessionSnapshot {
    let identity = ModelIdentity::new("openai", "responses", "gpt-5");
    SessionSnapshot::new(
        SessionId::from_str("session-1").unwrap(),
        Revision::from_u64(4),
        vec![Message::AbortedAssistant(Box::new(AbortedAssistant {
            content: vec![ContentBlock::Text("partial".into())],
            reasoning: "raw reasoning must not persist".into(),
            provider_context: vec![ProviderContextBlock {
                identity: identity.clone(),
                kind: "encrypted_reasoning".into(),
                position: Some(0),
                data: json!({"encrypted": "opaque"}),
            }],
            ..AbortedAssistant::default()
        }))],
        identity,
        crate::CompactionState::default(),
    )
}

#[test]
fn snapshot_json_round_trip_preserves_replay_context_but_omits_raw_reasoning() {
    let snapshot = snapshot().with_metadata("title", "test session");

    let json = snapshot.to_json().unwrap();
    let decoded = SessionSnapshot::from_json(&json).unwrap();

    assert_eq!(decoded, snapshot);
    assert_eq!(decoded.schema_version(), SESSION_SNAPSHOT_SCHEMA_VERSION);
    assert_eq!(
        decoded.metadata().get("title").map(String::as_str),
        Some("test session")
    );
    assert!(matches!(
        decoded.history(),
        [Message::AbortedAssistant(message)]
            if message.reasoning.is_empty() && message.provider_context.len() == 1
    ));
}

#[test]
fn every_supported_snapshot_fixture_migrates_to_the_current_schema() {
    for (version, fixture, expected_cache_key) in [
        (MIN_SESSION_SNAPSHOT_SCHEMA_VERSION, SNAPSHOT_V1, None),
        (
            SESSION_SNAPSHOT_SCHEMA_VERSION,
            SNAPSHOT_V2,
            Some("rho:fixture-cache"),
        ),
    ] {
        let original: serde_json::Value = serde_json::from_str(fixture).unwrap();
        assert_eq!(original["schema_version"], version);

        let snapshot = SessionSnapshot::from_json(fixture).unwrap();
        let migrated: serde_json::Value =
            serde_json::from_str(&snapshot.to_json().unwrap()).unwrap();

        assert_eq!(snapshot.schema_version(), SESSION_SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(migrated["schema_version"], SESSION_SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.prompt_cache_key(), expected_cache_key);
        assert_eq!(snapshot.history().len(), 2);
    }
}

#[test]
fn malformed_and_out_of_range_snapshot_schemas_are_rejected() {
    for json in [
        "{}".to_string(),
        SNAPSHOT_V1.replace("\"schema_version\": 1", "\"schema_version\": 0"),
        SNAPSHOT_V2.replace("\"schema_version\": 2", "\"schema_version\": 3"),
        SNAPSHOT_V2.replace(
            "\"session_id\": \"22222222-2222-4222-8222-222222222222\",",
            "",
        ),
    ] {
        assert!(
            SessionSnapshot::from_json(&json).is_err(),
            "accepted {json}"
        );
        assert!(serde_json::from_str::<SessionSnapshot>(&json).is_err());
    }
}

#[tokio::test]
async fn public_store_boundary_loads_and_atomically_replaces_snapshots() {
    let store = InMemorySessionStore::new();
    let first = snapshot();
    let id = first.session_id().clone();
    let second = SessionSnapshot::new(
        id.clone(),
        Revision::from_u64(5),
        vec![Message::user_text("replacement")],
        first.provider().clone(),
        crate::CompactionState::default(),
    )
    .with_prompt_cache_key("rho:session-1");

    SessionStore::save(&store, first).await.unwrap();
    SessionStore::save(&store, second.clone()).await.unwrap();

    assert_eq!(SessionStore::load(&store, &id).await.unwrap(), Some(second));
}

#[test]
fn unsupported_snapshot_schema_is_rejected() {
    let mut value = serde_json::to_value(snapshot()).unwrap();
    value["schema_version"] = json!(SESSION_SNAPSHOT_SCHEMA_VERSION + 1);

    let error = SessionSnapshot::from_json(&value.to_string()).unwrap_err();

    assert!(error
        .to_string()
        .contains("unsupported session snapshot schema"));
}

#[test]
fn in_memory_store_replaces_complete_snapshots_atomically() {
    let store = InMemorySessionStore::new();
    let first = snapshot();
    let id = first.session_id().clone();
    let second = SessionSnapshot::new(
        id.clone(),
        Revision::from_u64(5),
        vec![Message::user_text("new revision")],
        first.provider().clone(),
        crate::CompactionState::default(),
    );

    assert_eq!(store.save(first.clone()), None);
    assert_eq!(store.save(second.clone()), Some(first));
    assert_eq!(store.load(&id), Some(second.clone()));
    assert_eq!(store.remove(&id), Some(second));
    assert!(store.is_empty());
}
