use pretty_assertions::assert_eq;

use super::ModeChangeError;
use crate::{
    app::session_runtime::test_support::idle_runtime, config::Config, permission::PermissionMode,
};

// Covers: auto without a configured classifier model must be refused as a
// request the requester can fix, and leave the session in its previous mode,
// matching headless startup.
// Owner: session runtime permission mode change
#[tokio::test]
async fn set_mode_auto_without_classifier_keeps_the_previous_mode() {
    let home = tempfile::tempdir().unwrap();
    let config = Config {
        provider: "test".into(),
        model: "model".into(),
        permission_mode: PermissionMode::Bypass,
        ..Config::default()
    };
    let mut runtime = idle_runtime(home.path(), &config).await;

    let error = runtime
        .set_permission_mode(PermissionMode::Auto, &config)
        .await
        .expect_err("auto needs a classifier model");

    assert!(matches!(error, ModeChangeError::Unsupported(_)), "{error}");
    assert_eq!(runtime.permission_mode(), PermissionMode::Bypass);
    runtime.shutdown().await;
}
