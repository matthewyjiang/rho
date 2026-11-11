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

// Covers: a daemon session's spend counts its pings, which the totals add as
// their own row. Whichever report came last, the two rows must sum to the
// spend, so pings from before an attach are not counted twice.
// Owner: App cache-warm wiring (pure cost derivation).
#[test]
fn a_daemon_sessions_main_cost_is_its_spend_less_its_pings_in_either_order() {
    let main_cost = |app: &crate::tui::App| {
        app.usage
            .cumulative_usage
            .as_ref()
            .and_then(|usage| usage.cost_usd_micros)
    };

    let mut spend_first = crate::tui::tests::test_app();
    spend_first.usage.daemon_spend = Some(Some(900));
    spend_first.sync_daemon_main_cost();
    spend_first.show_daemon_cache_warm(None, 300);

    let mut pings_first = crate::tui::tests::test_app();
    pings_first.show_daemon_cache_warm(None, 300);
    pings_first.usage.daemon_spend = Some(Some(900));
    pings_first.sync_daemon_main_cost();

    assert_eq!(
        [main_cost(&spend_first), main_cost(&pings_first)],
        [Some(600), Some(600)]
    );
}
