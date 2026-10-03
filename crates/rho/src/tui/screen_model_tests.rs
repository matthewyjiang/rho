use pretty_assertions::assert_eq;

use crate::{
    app::config_repository::ConfigRepository, config::InternalAgentModelConfig,
    permission_classifier::DECISION_SCREEN_ID,
};

/// A screen entry on a decision host, with that host's auth.
fn screen(provider: &str, model: &str, percent: Option<u8>) -> InternalAgentModelConfig {
    let auth = match provider {
        "typesafe" => "typesafe-api-key",
        _ => "none",
    };
    let mut selection = InternalAgentModelConfig::new(provider.into(), model.into(), auth.into());
    selection.expect_rho_mut().allow_threshold_percent = percent;
    selection
}

// Covers: saving the allow threshold writes the session's whole screen entry,
// so the saved config names the screen the user edited even when it held no
// screen or an older one (after a failed model save, or an outside edit). A
// failed save changes neither the config nor the session. Only a saved change
// marks the classifier for sync.
// Owner: permission screen threshold commit
#[test]
fn allow_threshold_saves_the_session_screen() {
    let jev = screen("typesafe", "jev-latest", None);
    let clef = screen("ollama", "clef", Some(97));
    // (name, saved screen, session screen, save fails, expected saved, expected session)
    let cases = [
        (
            "same entry",
            Some(jev.clone()),
            Some(jev.clone()),
            false,
            Some(screen("typesafe", "jev-latest", Some(90))),
            Some(screen("typesafe", "jev-latest", Some(90))),
        ),
        (
            "no saved entry",
            None,
            Some(jev.clone()),
            false,
            Some(screen("typesafe", "jev-latest", Some(90))),
            Some(screen("typesafe", "jev-latest", Some(90))),
        ),
        (
            "older saved entry",
            Some(clef.clone()),
            Some(jev.clone()),
            false,
            Some(screen("typesafe", "jev-latest", Some(90))),
            Some(screen("typesafe", "jev-latest", Some(90))),
        ),
        (
            "save fails",
            Some(jev.clone()),
            Some(jev.clone()),
            true,
            Some(jev.clone()),
            Some(jev.clone()),
        ),
        ("no session screen", None, None, false, None, None),
    ];
    for (name, saved, session, fails, expected_saved, expected_session) in cases {
        let mut app = crate::tui::tests::test_app();
        app.info.services.config_repository = ConfigRepository::temporary_for_tests().unwrap();
        app.info
            .services
            .config_repository
            .update(|config| {
                if let Some(saved) = saved.clone() {
                    config.set_internal_agent_model_config(DECISION_SCREEN_ID, saved);
                }
            })
            .unwrap();
        app.info.runtime.internal_agents.remove(DECISION_SCREEN_ID);
        if let Some(session) = session.clone() {
            app.info
                .runtime
                .internal_agents
                .insert(DECISION_SCREEN_ID.into(), session);
        }
        if fails {
            app.info
                .services
                .config_repository
                .fail_next_save_for_tests();
        }

        app.store_allow_threshold(90);

        let reloaded = app.info.services.config_repository.load().unwrap();
        let synced = expected_session != session;
        assert_eq!(
            (
                reloaded.internal_agent_model(DECISION_SCREEN_ID).cloned(),
                app.info
                    .runtime
                    .internal_agents
                    .get(DECISION_SCREEN_ID)
                    .cloned(),
                app.classifier_config_sync_pending,
            ),
            (expected_saved, expected_session, synced),
            "{name}"
        );
    }
}
