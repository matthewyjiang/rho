use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelEvent, ModelResponse, ModelUsage},
    provider::ScriptedTurn,
    UserInput,
};

use crate::{
    app::interactive_runtime::test_runtime, compaction::CompactionConfig, tui::tests::test_app,
};

// Covers: a saved edit replaces an in-flight policy, gets lost on settlement,
// or discards calibration. Owner: host session lifecycle. PTY covers the picker
// and visible auto-compact; this test controls the busy boundary directly.
#[tokio::test]
async fn compaction_edits_wait_for_idle_and_preserve_calibration() {
    let mut app = test_app();
    let mut agent = test_runtime(vec![ScriptedTurn::streaming(
        vec![ModelEvent::Usage(ModelUsage {
            input_tokens: Some(100_000),
            ..ModelUsage::default()
        })],
        ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
    )])
    .await;
    agent.set_context_window(Some(131_072)).unwrap();
    agent
        .set_compaction_config(CompactionConfig {
            auto_compact: true,
            threshold_percent: 95,
            target_percent: 50,
        })
        .unwrap();
    agent
        .start(UserInput::text("history".repeat(300)), None)
        .await
        .unwrap();

    app.info
        .services
        .config_repository
        .update(|config| {
            config.auto_compact = true;
            config.set_compact_threshold_percent(75);
            config.set_compact_target_percent(25);
        })
        .unwrap();
    app.compaction_reload_pending = true;
    assert!(!app.apply_pending_compaction_config(&mut agent).unwrap());
    assert!(app.compaction_reload_pending);

    agent.finish_run().await.unwrap();
    assert!(!agent.should_auto_compact());
    let calibrated = agent.take_context_usage().unwrap();
    let history = agent.history();
    assert!(app.apply_pending_compaction_config(&mut agent).unwrap());
    assert!(!app.compaction_reload_pending);
    assert_eq!(agent.take_context_usage(), Some(calibrated));
    assert_eq!(agent.history(), history);
    assert!(agent.should_auto_compact());
}
