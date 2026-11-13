// Covers: a session's spend counts its pings, which the totals add as
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
