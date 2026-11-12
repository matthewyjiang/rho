use pretty_assertions::assert_eq;
use rho_sdk::model::ServiceTier;

use super::SavedSettings;
use crate::{
    app::session_runtime::{
        test_support::{idle_built, idle_runtime},
        SessionRuntime, SessionStorage,
    },
    config::Config,
    permission::PermissionMode,
    session::Session as StoredSession,
};

// Covers: a reopened session takes back the settings its file keeps,
// including a fast-mode preference its model cannot use, which a model
// switch keeps and a switch back to a fast model shows again.
// Owner: session runtime saved settings
#[tokio::test]
async fn a_reopened_session_restores_its_saved_settings() {
    let root = tempfile::tempdir().unwrap();
    let config = Config {
        provider: "test".into(),
        model: "model".into(),
        auth: "api-key".into(),
        ..Config::default()
    };
    let mut saved = idle_runtime(root.path(), &config).await;
    saved
        .built
        .session
        .set_service_tier(Some(ServiceTier::Priority))
        .unwrap();
    saved
        .set_permission_mode(PermissionMode::AllowEdits, &config)
        .await
        .unwrap();
    saved.keep_settings().unwrap();

    let id = saved.session_id().clone();
    let (stored, _) =
        StoredSession::open_by_id_in_root(root.path(), root.path(), id.as_str()).unwrap();
    let mut reopened = SessionRuntime::for_tests(
        idle_built(&config, id).await,
        SessionStorage::saved(stored),
        &config,
    );
    reopened.restore_settings(&config).await;

    assert_eq!(SavedSettings::of(&reopened), SavedSettings::of(&saved));
}
