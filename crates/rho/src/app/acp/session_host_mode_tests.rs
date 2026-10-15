use std::sync::Arc;

use agent_client_protocol::ErrorCode;
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{Message, ModelIdentity},
    provider::{ModelProvider, ScriptedProvider},
    SessionOptions,
};

use super::{super::SessionHost, SessionPermission};
use crate::{
    app::session_assembly::BuiltSession, config::Config, diagnostics::RuntimeDiagnostics,
    herdr::HerdrReporter, permission::PermissionMode, session::Session as StoredSession,
    tools::sdk_registry::AppToolSet,
};
use agent_client_protocol::schema::v1::SessionId;

fn test_config() -> Config {
    Config {
        provider: "test".into(),
        model: "model".into(),
        permission_mode: PermissionMode::Bypass,
        ..Config::default()
    }
}

async fn host(home: &std::path::Path, config: &Config) -> SessionHost {
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        Vec::new(),
    ));
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::new()).await.unwrap();
    session
        .append_message(Message::user_text("retained conversation"))
        .unwrap();
    let built = BuiltSession {
        runtime,
        session,
        provider,
        tools: AppToolSet::disabled(),
        hooks: None,
        approval_receiver: None,
        prompt_template: None,
        prompt: Default::default(),
        diagnostics: RuntimeDiagnostics::new(config),
    };
    let stored = StoredSession::create_in_root(home, home).unwrap();
    SessionHost::from_built(
        SessionId::new(stored.id()),
        built,
        stored,
        Default::default(),
        SessionPermission {
            mode: config.permission_mode,
            writes: Default::default(),
            workspace_root: home.to_path_buf(),
        },
        config.auth.clone(),
        HerdrReporter::default(),
    )
}

// Covers: session/set_mode must swap the runtime's approval wiring for the new
// mode while keeping the live conversation and session identity, and modes
// that answer through the host must get an approval channel.
// Owner: ACP session transaction
#[tokio::test]
async fn set_mode_rebuilds_policy_and_keeps_the_conversation() {
    let home = tempfile::tempdir().unwrap();
    let config = test_config();
    let mut host = host(home.path(), &config).await;
    let session_id = host.built.session.id().clone();
    let history = host.built.session.history();

    for (mode, host_answers) in [
        (PermissionMode::Plan, false),
        (PermissionMode::Supervised, true),
        (PermissionMode::AllowEdits, true),
        (PermissionMode::Bypass, false),
    ] {
        host.set_permission_mode(mode, &config).await.unwrap();
        assert_eq!(host.permission.mode, mode);
        assert_eq!(
            host.built.approval_receiver.is_some(),
            host_answers,
            "{mode:?}"
        );
        assert_eq!(host.built.session.id(), &session_id, "{mode:?}");
        assert_eq!(host.built.session.history(), history, "{mode:?}");
    }
    host.shutdown().await;
}

// Covers: auto without a configured classifier model must be rejected as bad
// params and leave the session in its previous mode, matching headless startup.
// Owner: ACP session transaction
#[tokio::test]
async fn set_mode_auto_without_classifier_keeps_the_previous_mode() {
    let home = tempfile::tempdir().unwrap();
    let config = test_config();
    let mut host = host(home.path(), &config).await;

    let error = host
        .set_permission_mode(PermissionMode::Auto, &config)
        .await
        .expect_err("auto needs a classifier model");

    assert_eq!(error.code, ErrorCode::InvalidParams);
    assert_eq!(host.permission.mode, PermissionMode::Bypass);
    host.shutdown().await;
}
