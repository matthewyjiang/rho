use std::sync::Arc;

use super::*;

#[test]
fn model_updates_are_shared_with_executor_clones() {
    let executor = AgentExecutor::new(Config::default(), PathBuf::new(), PathBuf::new());
    let cloned = executor.clone();

    executor.update_model("openai-codex", "gpt-5.6-luna", rho_sdk::ReasoningLevel::Low);

    let config = cloned.config.read().expect("delegated config lock");
    assert_eq!(config.provider, "openai-codex");
    assert_eq!(config.model, "gpt-5.6-luna");
    assert_eq!(config.reasoning, rho_sdk::ReasoningLevel::Low);
}

#[tokio::test]
async fn cancellation_interrupts_concurrency_queue() {
    let permits = Arc::new(tokio::sync::Semaphore::new(0));
    let cancellation = RunCancellation::new();
    let queued = tokio::spawn({
        let permits = Arc::clone(&permits);
        let cancellation = cancellation.clone();
        async move { acquire_permit_or_cancel(permits, &cancellation).await }
    });

    cancellation.cancel();

    let permit = tokio::time::timeout(std::time::Duration::from_secs(1), queued)
        .await
        .expect("queued acquisition should observe cancellation")
        .unwrap()
        .unwrap();
    assert!(permit.is_none());
}

#[tokio::test]
async fn cancellation_wins_when_a_permit_is_already_available() {
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let cancellation = RunCancellation::new();
    cancellation.cancel();

    let permit = acquire_permit_or_cancel(permits, &cancellation)
        .await
        .unwrap();

    assert!(permit.is_none());
}
