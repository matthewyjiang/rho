use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ToolCall},
    provider::{ModelProvider, ScriptedProvider},
    SessionOptions,
};

use super::{HostStorage, SaveScope, TurnDisplay};
use crate::{
    app::{session_assembly::BuiltSession, session_runtime::SessionRuntime},
    config::Config,
    diagnostics::RuntimeDiagnostics,
    session::Session as StoredSession,
    tools::sdk_registry::AppToolSet,
};

/// A runtime on `provider` and `tools` whose session `id` is saved through
/// `storage`.
async fn runtime(
    id: &str,
    storage: HostStorage,
    provider: Arc<dyn ModelProvider>,
    tools: AppToolSet,
) -> SessionRuntime {
    let config = Config::default();
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .unwrap();
    let session = runtime
        .session(SessionOptions::new().id(rho_sdk::SessionId::from_string(id).unwrap()))
        .await
        .unwrap();
    let built = BuiltSession {
        runtime,
        session,
        provider,
        tools,
        hooks: None,
        approval_receiver: None,
        prompt_template: None,
        prompt: Default::default(),
        diagnostics: RuntimeDiagnostics::new(&config),
    };
    SessionRuntime::for_tests(built, storage, &config)
}

// Covers: both mid-turn checkpoints and settled saves restore loaded tool
// schemas in a fresh tool host, even when MCP inventory arrives after restore.
// Owner: session runtime persistence, independent of provider/credential startup.
#[tokio::test]
async fn saves_retain_the_loaded_tool_schemas() {
    use crate::tools::sdk_registry::StaticToolBundle;
    use rho_sdk::{
        model::ToolSpec,
        tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput},
        SessionStore, UserInput,
    };
    let name = "mcp__docs__lookup";
    let bundle = || {
        StaticToolBundle::new(vec![Arc::new(ScriptedTool::new(
            ToolSpec {
                name: name.into(),
                description: "docs".into(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            ScriptedToolOutcome::Success(ToolOutput::text("docs")),
        ))])
    };
    for scope in [SaveScope::Step, SaveScope::Settle] {
        let home = tempfile::tempdir().unwrap();
        let stored = StoredSession::create_in_root(home.path(), home.path()).unwrap();
        let id = stored.id().to_owned();
        let provider = Arc::new(ScriptedProvider::new(
            ModelIdentity::new("test", "test", "model"),
            [],
        ));
        let mut tools = AppToolSet::disabled();
        tools.add_bundle(bundle());
        let mut host = runtime(&id, HostStorage::saved(stored), provider.clone(), tools).await;
        let input = UserInput::text("load docs");
        let checkpoint = host.install_turn_checkpoint(&input, &input).unwrap();
        host.built
            .session
            .append_message(Message::user_text("load docs"))
            .unwrap();
        host.built.tools.code_mode().search_and_load(name, 1);
        match scope {
            SaveScope::Step => checkpoint
                .0
                .save(host.built.session.snapshot())
                .await
                .unwrap(),
            SaveScope::Settle => host
                .persist_turn(&checkpoint, /*assistant_text*/ None)
                .unwrap(),
        }
        host.shutdown().await;

        let (stored, _) =
            StoredSession::open_by_id_with_histories_in_root(home.path(), home.path(), &id)
                .unwrap();
        let snapshot = stored
            .snapshot_for_resume(provider.identity(), "restored".into())
            .unwrap();
        let mut restored = AppToolSet::disabled();
        restored.code_mode().restore(&snapshot);
        // A newly connected MCP bundle must not erase restored names.
        restored.add_bundle(bundle());
        assert_eq!(
            restored
                .specs()
                .iter()
                .map(|spec| spec.name.as_str())
                .collect::<Vec<_>>(),
            vec![name],
            "{scope:?}"
        );
    }
}

// Covers: a compaction that rewrites history before or during the turn must
// not make display copy unrelated rows, even once history grows past the
// turn's old position; the turn then shows its final reply instead.
// Owner: session runtime turn persistence
#[test]
fn turn_display_follows_only_history_that_extends_the_turn() {
    let earlier = Message::user_text("earlier");
    let input = Message::user_text("input");
    let reply = Message::assistant_text("reply");
    let summary = Message::user_text("summary");
    let later = Message::assistant_text("later");

    let shown = Message::user_text("shown");

    // Compacted mid-turn, after a save wrote the reply.
    let mut display = TurnDisplay::new(shown.clone(), input.clone(), 1);
    let extended = vec![earlier.clone(), input.clone(), reply.clone()];
    let pending = display.pending_rows(&extended, SaveScope::Step);
    assert_eq!(pending.rows, vec![shown.clone(), reply.clone()]);
    display.mark_written(pending);
    let compacted = vec![summary.clone(), reply.clone(), later.clone()];
    assert!(!display.follows(&compacted));
    assert_eq!(
        display.pending_rows(&compacted, SaveScope::Settle).rows,
        Vec::<Message>::new()
    );

    // Compacted before the first save, then grown past the turn's position.
    let mut display = TurnDisplay::new(shown.clone(), input, 1);
    for (history, written) in [
        (vec![summary.clone()], vec![shown]),
        (vec![summary, reply, later.clone(), later], Vec::new()),
    ] {
        assert!(!display.follows(&history));
        let pending = display.pending_rows(&history, SaveScope::Step);
        assert_eq!(pending.rows, written);
        display.mark_written(pending);
    }
}

// Covers: a refusal rolls back only a turn that produced nothing but model
// replies; a turn that ran tools or took other input keeps its history, and a
// compacted turn cannot be cut at its old position.
// Owner: session runtime turn persistence
#[test]
fn only_a_prompt_answered_by_replies_alone_rolls_back() {
    let earlier = Message::user_text("earlier");
    let input = Message::user_text("input");
    let reply = Message::assistant_text("refused");
    let call = Message::Assistant(vec![ContentBlock::ToolCall(ToolCall::new(
        "call".into(),
        "read_file".into(),
        serde_json::json!({}),
    ))]);
    let result = Message::ToolResult(rho_sdk::model::ToolResult::succeeded("call", "file"));
    let steer = Message::user_text("steer");
    let display = TurnDisplay::new(input.clone(), input.clone(), 1);

    for (history, expected) in [
        (vec![earlier.clone(), input.clone(), reply.clone()], Some(1)),
        (
            vec![earlier.clone(), input.clone(), call, result, reply.clone()],
            None,
        ),
        (
            vec![earlier.clone(), input.clone(), steer, reply.clone()],
            None,
        ),
        (vec![Message::user_text("summary"), reply], None),
    ] {
        assert_eq!(display.prompt_only_start(&history), expected, "{history:?}");
    }
}

// Covers: a background result reaches history before the pump records the
// row shown for it, so a step save must wait for that row rather than write
// the model's copy; once recorded, the shown row takes its place in order.
// Owner: session runtime turn persistence
#[test]
fn turn_display_shows_recorded_rows_for_background_results() {
    let input = Message::user_text("input");
    let result = Message::user_text("<background result>");
    let card = Message::System("card".into());
    let reply = Message::assistant_text("reply");
    let history = vec![input.clone(), result.clone(), reply.clone()];
    let mut display = TurnDisplay::new(input.clone(), input.clone(), 0);

    let pending = display.pending_rows(&history, SaveScope::Step);
    assert_eq!(pending.rows, vec![input]);
    display.mark_written(pending);

    display.applied.push_back((result, card.clone()));
    let pending = display.pending_rows(&history, SaveScope::Step);
    assert_eq!(pending.rows, vec![card, reply]);
    display.mark_written(pending);
    assert_eq!(
        display.pending_rows(&history, SaveScope::Settle).rows,
        Vec::<Message>::new()
    );
}
