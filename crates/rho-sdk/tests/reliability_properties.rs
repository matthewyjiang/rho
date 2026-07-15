mod support;

use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{
        AbortedAssistant, AssistantMessage, ContentBlock, ImageContent, Message, ModelEvent,
        ModelIdentity, ModelRequest, ModelUsage, PartialToolCall, ProviderContextBlock, ToolCall,
        ToolResult,
    },
    provider::{ModelProvider, ProviderEventSender, ProviderFuture},
    Error, RunEvent, SessionSnapshot, Workspace, SESSION_SNAPSHOT_SCHEMA_VERSION,
};
use serde_json::{json, Value};

use support::{identity, text_response, TEST_TIMEOUT};

const CASES: usize = 1_024;
const SEED: u64 = 0x2560_5eed_cafe_f00d;

struct DeterministicRng(u64);

impl DeterministicRng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn usize(&mut self, upper: usize) -> usize {
        (self.next() as usize) % upper
    }

    fn string(&mut self, max_chars: usize) -> String {
        const CHARS: [char; 12] = [
            '\0', '\n', '\\', '"', 'a', 'Z', '0', '/', 'é', '中', '🦀', '\u{2028}',
        ];
        let len = self.usize(max_chars + 1);
        (0..len).map(|_| CHARS[self.usize(CHARS.len())]).collect()
    }

    fn optional_string(&mut self) -> Option<String> {
        (self.usize(3) != 0).then(|| self.string(24))
    }

    fn value(&mut self) -> Value {
        match self.usize(6) {
            0 => Value::Null,
            1 => Value::Bool(self.usize(2) == 0),
            2 => json!(self.next()),
            3 => json!(self.string(24)),
            4 => json!([self.string(8), self.next()]),
            _ => json!({"key": self.string(16), "n": self.next()}),
        }
    }
}

fn model_identity(rng: &mut DeterministicRng) -> ModelIdentity {
    ModelIdentity::new(rng.string(12), rng.string(12), rng.string(12))
}

fn content(rng: &mut DeterministicRng) -> ContentBlock {
    match rng.usize(3) {
        0 => ContentBlock::Text(rng.string(80)),
        1 => ContentBlock::Image(ImageContent {
            data: rng.string(48),
            mime_type: rng.string(16),
        }),
        _ => ContentBlock::ToolCall(ToolCall {
            id: rng.string(16),
            name: rng.string(16),
            arguments: rng.value(),
        }),
    }
}

fn content_list(rng: &mut DeterministicRng) -> Vec<ContentBlock> {
    let len = rng.usize(6);
    (0..len).map(|_| content(rng)).collect()
}

fn message(rng: &mut DeterministicRng) -> Message {
    match rng.usize(6) {
        0 => Message::System(rng.string(80)),
        1 => Message::User(content_list(rng)),
        2 => Message::Assistant(content_list(rng)),
        3 => Message::assistant(AssistantMessage {
            content: content_list(rng),
            provenance: Some(model_identity(rng)),
            reasoning_summary: rng.optional_string(),
            provider_context: vec![ProviderContextBlock {
                identity: model_identity(rng),
                kind: rng.string(16),
                position: Some(rng.usize(32)),
                data: rng.value(),
            }],
        }),
        4 => Message::AbortedAssistant(Box::new(AbortedAssistant {
            content: content_list(rng),
            reasoning: rng.string(32),
            provenance: Some(model_identity(rng)),
            reasoning_summary: rng.optional_string(),
            provider_context: Vec::new(),
            tool_calls: vec![PartialToolCall {
                id: rng.optional_string(),
                name: rng.optional_string(),
                arguments: rng.string(48),
            }],
            usage: ModelUsage {
                total_tokens: Some(rng.next()),
                ..ModelUsage::default()
            },
        })),
        _ => Message::ToolResult(ToolResult {
            id: rng.string(16),
            ok: rng.usize(2) == 0,
            content: rng.string(80),
        }),
    }
}

#[test]
fn deterministic_message_deserialization_round_trips_arbitrary_content() {
    let mut rng = DeterministicRng(SEED);
    for case in 0..CASES {
        let expected = message(&mut rng);
        let encoded = serde_json::to_vec(&expected).unwrap();
        let actual: Message = serde_json::from_slice(&encoded)
            .unwrap_or_else(|error| panic!("seed {SEED:#x}, case {case}: {error}"));
        assert_eq!(actual, expected, "seed {SEED:#x}, case {case}");
    }
}

#[test]
fn deterministic_malformed_message_and_snapshot_inputs_never_panic() {
    let mut rng = DeterministicRng(SEED ^ 0x00a1_1ce5);
    for case in 0..CASES {
        let bytes = rng.string(128);
        let _ = serde_json::from_str::<Message>(&bytes);
        let _ = SessionSnapshot::from_json(&bytes);

        let malformed = json!({
            "schema_version": rng.next(),
            "session_id": rng.string(12),
            "revision": rng.next(),
            "history": [rng.value()],
            "provider": rng.value(),
            "metadata": rng.value(),
        });
        let result = SessionSnapshot::from_json(&malformed.to_string());
        if let Ok(snapshot) = result {
            assert_eq!(
                snapshot.schema_version(),
                SESSION_SNAPSHOT_SCHEMA_VERSION,
                "seed {SEED:#x}, case {case}"
            );
        }
    }
}

#[test]
fn snapshot_schema_rejects_missing_invalid_and_future_contract_fields() {
    let valid = json!({
        "schema_version": SESSION_SNAPSHOT_SCHEMA_VERSION,
        "session_id": "property-session",
        "revision": 0,
        "history": [],
        "provider": {"provider": "test", "api": "scripted", "model": "v1"},
    });
    assert!(SessionSnapshot::from_json(&valid.to_string()).is_ok());

    for field in [
        "schema_version",
        "session_id",
        "revision",
        "history",
        "provider",
    ] {
        let mut invalid = valid.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(
            SessionSnapshot::from_json(&invalid.to_string()).is_err(),
            "required schema field {field} was accepted"
        );
    }

    for version in [0, SESSION_SNAPSHOT_SCHEMA_VERSION + 1, u32::MAX] {
        let mut invalid = valid.clone();
        invalid["schema_version"] = json!(version);
        assert!(SessionSnapshot::from_json(&invalid.to_string()).is_err());
    }
}

#[derive(Clone)]
struct FragmentProvider {
    fragments: Vec<(usize, Option<String>, Option<String>, String)>,
}

impl ModelProvider for FragmentProvider {
    fn identity(&self) -> ModelIdentity {
        identity()
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async { Ok(text_response("unused")) })
    }

    fn send_turn_stream<'a>(
        &'a self,
        request: ModelRequest<'a>,
        events: ProviderEventSender,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            for (index, id, name, arguments) in &self.fragments {
                events
                    .send(ModelEvent::ToolCallDelta {
                        index: *index,
                        id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    })
                    .await?;
            }
            request.cancellation.cancelled().await;
            Err(rho_sdk::ProviderError::interrupted("cancelled"))
        })
    }
}

#[tokio::test]
async fn streamed_tool_call_fragments_assemble_by_index_for_every_partition() {
    let original = [
        (2, "call-2", "write", "{\"b\":2}"),
        (0, "call-0", "read", "{\"a\":1}"),
    ];
    for split_bias in 1..8 {
        let mut fragments = Vec::new();
        let mut expected = BTreeMap::new();
        for (index, id, name, arguments) in original {
            let mut offset = 0;
            let mut first = true;
            while offset < arguments.len() {
                let width = (split_bias + offset).min(arguments.len() - offset).max(1);
                let end = (offset + width).min(arguments.len());
                fragments.push((
                    index,
                    first.then(|| id.to_owned()),
                    first.then(|| name.to_owned()),
                    arguments[offset..end].to_owned(),
                ));
                first = false;
                offset = end;
            }
            expected.insert(index, (id, name, arguments));
        }

        let session = support::session_with(FragmentProvider {
            fragments: fragments.clone(),
        })
        .await;
        let mut run = session
            .start(rho_sdk::UserInput::text("assemble"))
            .await
            .unwrap();
        let mut observed = BTreeMap::<usize, String>::new();
        while observed.values().map(String::len).sum::<usize>()
            < fragments.iter().map(|fragment| fragment.3.len()).sum()
        {
            let event = tokio::time::timeout(TEST_TIMEOUT, run.next_event())
                .await
                .unwrap()
                .unwrap();
            if let RunEvent::ToolCallUpdated {
                index,
                arguments_delta,
                ..
            } = event
            {
                observed
                    .entry(index)
                    .or_default()
                    .push_str(&arguments_delta);
            }
        }
        run.cancel();
        assert!(matches!(run.outcome().await, Err(Error::Cancelled)));

        let history = session.history();
        let partials = match history.as_slice() {
            [Message::User(_), Message::AbortedAssistant(assistant)] => &assistant.tool_calls,
            history => panic!("unexpected cancelled history: {history:?}"),
        };
        assert_eq!(partials.len(), expected.len());
        for (partial, (_, (id, name, arguments))) in partials.iter().zip(expected.iter()) {
            assert_eq!(partial.id.as_deref(), Some(*id));
            assert_eq!(partial.name.as_deref(), Some(*name));
            assert_eq!(&partial.arguments, arguments);
        }
        assert_eq!(observed[&0], "{\"a\":1}");
        assert_eq!(observed[&2], "{\"b\":2}");
    }
}

#[test]
fn deterministic_path_policy_inputs_never_escape_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(root.path()).unwrap();
    let mut rng = DeterministicRng(SEED ^ 0x5afe);

    for _case in 0..CASES {
        let parts = (0..=rng.usize(8))
            .map(|_| match rng.usize(5) {
                0 => "..".to_owned(),
                1 => ".".to_owned(),
                _ => format!("part-{}", rng.next()),
            })
            .collect::<Vec<_>>();
        let candidate = parts.join("/");
        match workspace.resolve(&candidate) {
            Ok(resolved) => {
                assert!(!parts.iter().any(|part| part == ".."));
                assert!(resolved.starts_with(workspace.root()));
            }
            Err(_) => assert!(parts.iter().any(|part| part == "..")),
        }
    }
}
