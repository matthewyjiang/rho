use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    UserInput,
};
use tokio::sync::mpsc;

use crate::{
    app::session_runtime::{
        test_support::{runtime_on, HangUntilCancel},
        Turn, TurnIo, TurnOutcome,
    },
    config::Config,
};

// Covers: only a turn that reached the model's answer asks to name the
// session; a cancelled one asks for nothing.
// Owner: session runtime titles
#[tokio::test]
async fn only_an_answered_turn_asks_for_a_title() {
    let config = Config {
        provider: "test".into(),
        model: "model".into(),
        ..Config::default()
    };
    let answers: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    ));
    let hangs: Arc<dyn ModelProvider> = Arc::new(HangUntilCancel);
    for (label, provider, cancel, answered) in [
        ("cancelled", hangs, true, false),
        ("answered", answers, false, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut runtime = runtime_on(root.path(), &config, provider).await;
        let prompt = runtime.begin_prompt(&config).await.unwrap();
        if cancel {
            runtime.handle().cancel();
        }
        let (events, _shown) = mpsc::unbounded_channel();
        let (_controls, control) = mpsc::unbounded_channel();
        let input = UserInput::text("name this session");
        let outcome = runtime
            .run_turn(
                &prompt,
                Turn {
                    display: input.clone(),
                    model: input,
                    calls: Vec::new(),
                },
                TurnIo { events, control },
                &config,
            )
            .await
            .unwrap();
        drop(prompt);
        assert_eq!(
            (
                matches!(outcome, TurnOutcome::Completed(_)),
                runtime.take_title().is_some(),
            ),
            (answered, answered),
            "{label}"
        );
        runtime.shutdown().await;
    }
}
