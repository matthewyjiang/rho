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
