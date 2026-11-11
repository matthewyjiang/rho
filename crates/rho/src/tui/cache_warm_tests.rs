use rho_providers::model::ModelUsage;

use crate::cache_warm::CacheWarmOutcome;

// Covers: `/clear` while a ping is in flight. The provider already billed the
// prefix read, so the send must finish and reach the usage ledger, while its
// result stays out of the new conversation.
// Owner: App cache-warm wiring (PTY cannot observe the detached send).
#[tokio::test]
async fn reset_lets_an_in_flight_ping_finish_unobserved() {
    let mut app = crate::tui::tests::test_app();
    let (release, gate) = tokio::sync::oneshot::channel::<()>();
    let (recorded, ledger) = tokio::sync::oneshot::channel::<()>();
    app.spawn_cache_warm_ping(async move {
        gate.await.unwrap();
        recorded.send(()).unwrap();
        CacheWarmOutcome::default()
    });

    app.reset_cache_warm();
    assert!(!app.tasks.has_pending());

    release.send(()).unwrap();
    assert_eq!(ledger.await, Ok(()));
    assert!(!app.apply_finished_ui_tasks());
}

// Covers: a provider switch while a ping is in flight. The ping was billed in
// this conversation, so its spend must reach the totals, but its result
// describes the old route and must not halt warming on the new one.
// Owner: App cache-warm wiring.
#[tokio::test]
async fn an_orphaned_ping_counts_its_spend_but_cannot_halt_the_new_route() {
    let mut app = crate::tui::tests::test_app();
    app.spawn_cache_warm_ping(async {
        CacheWarmOutcome {
            usage: ModelUsage {
                cost_usd_micros: Some(700),
                ..ModelUsage::default()
            },
            error: Some("connection reset".into()),
        }
    });
    app.cache_warm_provider_replaced();
    while !app.tasks.has_finished() {
        tokio::task::yield_now().await;
    }

    assert!(!app.apply_finished_ui_tasks());
    assert_eq!(app.usage.extra.cache_warm, 700);
    assert_eq!(app.usage.cache_warm.halt(), None);
}
