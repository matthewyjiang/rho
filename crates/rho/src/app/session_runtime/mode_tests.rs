use pretty_assertions::assert_eq;

use super::SettingChangeError;
use crate::{
    app::session_runtime::test_support::idle_runtime,
    config::{Config, InternalAgentModelConfig},
    permission::PermissionMode,
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

    assert!(
        matches!(error, SettingChangeError::Unsupported(_)),
        "{error}"
    );
    assert_eq!(runtime.permission_mode(), PermissionMode::Bypass);
    runtime.shutdown().await;
}

// Covers: a rebuild for another reason, such as turning the advisor on with
// the session's opening config, keeps the approvals of the mode the session
// runs in now. Wired from the stale opening mode, a supervised session under
// a bypass config would deny every approval, and one under an auto config
// would let the classifier approve what the person asked to approve.
// Owner: session runtime rebuild
#[tokio::test]
async fn advisor_rebuild_keeps_the_session_mode_approvals() {
    let home = tempfile::tempdir().unwrap();
    let opening = Config {
        provider: "test".into(),
        model: "model".into(),
        permission_mode: PermissionMode::Bypass,
        ..Config::default()
    };
    let mut runtime = idle_runtime(home.path(), &opening).await;
    runtime.built.tools = crate::tools::sdk_registry::AppToolSet::new(
        &opening,
        runtime.built.diagnostics.clone(),
        crate::tools::sdk_registry::ToolSetOptions::new(crate::agent::AgentCapabilities::new(
            [crate::agent::ToolCapability::Advisor]
                .into_iter()
                .collect(),
        ))
        .advisor(crate::tools::advisor::AdvisorSessionStore::new()),
    );
    runtime
        .set_permission_mode(PermissionMode::Supervised, &opening)
        .await
        .unwrap();

    runtime
        .set_advisor(
            Some(InternalAgentModelConfig::new(
                "test".into(),
                "advisor".into(),
                "none".into(),
            )),
            &opening,
        )
        .await
        .unwrap();

    assert_eq!(
        (
            runtime.permission_mode(),
            runtime.built.approval_receiver.is_some(),
            runtime.built.tools.advisor_registered(),
        ),
        (PermissionMode::Supervised, true, true)
    );
    runtime.shutdown().await;
}
