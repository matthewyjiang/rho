use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

// Covers: a prompt carries its message as content blocks, every kind in its
// wire form, with absent optionals left out (#1670).
// Owner: protocol wire contract.
#[test]
fn a_prompt_carries_its_message_as_content_blocks() {
    let params = SessionPromptParams {
        author: PromptAuthor::Person,
        session: SessionId::new("s1"),
        content: vec![
            ContentBlock::Text {
                text: "look at these".into(),
            },
            ContentBlock::Image {
                mime_type: "image/png".into(),
                data: "aGk=".into(),
            },
            ContentBlock::Resource {
                uri: "file:///w/a.rs".into(),
                text: "fn a() {}".into(),
            },
            ContentBlock::ResourceLink {
                uri: "file:///w/notes.md".into(),
                name: "notes.md".into(),
                description: Some("meeting notes".into()),
            },
            ContentBlock::ResourceLink {
                uri: "file:///w/c".into(),
                name: "c".into(),
                description: None,
            },
        ],
        timeout_ms: None,
        environment: ClientEnvironment::from_iter([("HOME".to_owned(), "/home/u".to_owned())]),
        idempotency_key: Some(IdempotencyKey::new("k1")),
        grant: None,
    };
    let wire = json!({
        "session": "s1",
        "content": [
            {"type": "text", "text": "look at these"},
            {"type": "image", "mime_type": "image/png", "data": "aGk="},
            {"type": "resource", "uri": "file:///w/a.rs", "text": "fn a() {}"},
            {
                "type": "resource_link",
                "uri": "file:///w/notes.md",
                "name": "notes.md",
                "description": "meeting notes",
            },
            {"type": "resource_link", "uri": "file:///w/c", "name": "c"},
        ],
        "environment": {"HOME": "/home/u"},
        "idempotency_key": "k1",
    });
    assert_eq!(serde_json::to_value(&params).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<SessionPromptParams>(wire).unwrap(),
        params
    );
}

// Covers: a client sends no variable the daemon would refuse its whole
// environment for. Windows keeps each drive's working directory under a
// name that holds `=`, so every session a client opened from `cmd.exe` was
// refused.
// Owner: protocol client environment.
#[test]
fn a_clients_environment_leaves_out_names_no_command_takes() {
    let environment = ClientEnvironment::of(
        [("PATH", r"C:\bin"), ("=C:", r"C:\work"), ("EMPTY", "")]
            .map(|(name, value)| (name.into(), value.into())),
    );

    assert_eq!(
        environment.iter().collect::<Vec<_>>(),
        [("EMPTY", ""), ("PATH", r"C:\bin")]
    );
}

// Covers: `rho/session/event` carries an event's time and notification
// flat beside `session`, `seq` and `event`, leaves them out when absent, and
// a client that reads only `SessionEventParams` reads the same notification,
// ignoring them.
// Owner: protocol wire contract.
#[test]
fn an_event_envelope_adds_its_time_and_notification_beside_the_params() {
    let params = SessionEventParams {
        session: SessionId::new("s1"),
        seq: Seq(7),
        event: SessionEvent::Notice {
            text: "hello".into(),
        },
    };
    let info = crate::display::NotificationInfo::new(
        crate::display::NotificationId::new("card-1"),
        crate::display::NotificationUrgency::Notable,
        crate::display::NotificationSubject::Other,
        Vec::new(),
    );
    let mut timed = SessionEventEnvelope::new(params.clone());
    timed.at = Some(1_764_158_400_123);
    timed.notification = Some(Box::new(info));
    let cases = [
        (
            SessionEventEnvelope::new(params.clone()),
            json!({
                "session": "s1",
                "seq": 7,
                "event": {"type": "notice", "text": "hello"},
            }),
        ),
        (
            timed,
            json!({
                "session": "s1",
                "seq": 7,
                "at": 1_764_158_400_123_u64,
                "event": {"type": "notice", "text": "hello"},
                "notification": {
                    "id": "card-1",
                    "urgency": "notable",
                    "subject": {"kind": "other"},
                    "items": [],
                },
            }),
        ),
    ];
    for (envelope, wire) in cases {
        assert_eq!(serde_json::to_value(&envelope).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<SessionEventEnvelope>(wire.clone()).unwrap(),
            envelope
        );
        assert_eq!(
            serde_json::from_value::<SessionEventParams>(wire).unwrap(),
            params
        );
    }
}
