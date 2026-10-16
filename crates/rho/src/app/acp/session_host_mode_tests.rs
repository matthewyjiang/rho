use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use agent_client_protocol::{
    schema::v1::{
        ContentBlock as AcpContentBlock, PromptRequest, RequestPermissionOutcome,
        RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
        SessionConfigKind, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
        ToolCallStatus,
    },
    Error as AcpError, ErrorCode,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall, ToolSpec},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    tool::{Tool, ToolContext, ToolError, ToolFuture, ToolInvocation, ToolOutput},
    CapabilityRequest, CapabilitySource, PathScope, SessionOptions,
};

use super::{super::SessionHost, SessionPermission};
use crate::{
    app::{acp::AcpClientPort, session_assembly::BuiltSession},
    config::Config,
    diagnostics::RuntimeDiagnostics,
    permission::PermissionMode,
    session::Session as StoredSession,
    tools::sdk_registry::{AppToolSet, StaticToolBundle},
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

async fn host(
    home: &std::path::Path,
    config: &Config,
    turns: Vec<ScriptedTurn>,
    tools: AppToolSet,
) -> SessionHost {
    let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        turns,
    ));
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .unwrap();
    let stored = StoredSession::create_in_root(home, home).unwrap();
    let session = runtime
        .session(SessionOptions::new().id(rho_sdk::SessionId::from_string(stored.id()).unwrap()))
        .await
        .unwrap();
    session
        .append_message(Message::user_text("retained conversation"))
        .unwrap();
    let built = BuiltSession {
        runtime,
        session,
        provider,
        tools,
        hooks: None,
        approval_receiver: None,
        prompt_template: None,
        prompt: Default::default(),
        diagnostics: RuntimeDiagnostics::new(config),
    };
    let startup = crate::app::acp::AcpStartup::for_tests(config.clone(), home);
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
        &startup,
        Vec::new(),
    )
}

/// Tool that asks for an in-workspace write through the real authorization path.
struct WriteProbe;

const PROBE: &str = "write_probe";

impl Tool for WriteProbe {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: PROBE.into(),
            description: "test tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        let path = context
            .workspace_root()
            .expect("the session has a workspace")
            .join("file.txt");
        Box::pin(async move {
            context
                .authorize(CapabilityRequest::write_path(
                    path,
                    PathScope::PrimaryWorkspace,
                    CapabilitySource::built_in_tool(PROBE),
                ))
                .await
                .map_err(|error| ToolError::policy_denied(&error))?;
            Ok(ToolOutput::text("wrote"))
        })
    }
}

/// Host that approves every permission prompt once and records what it saw.
#[derive(Default)]
struct ApprovingPort {
    prompts: Mutex<usize>,
    statuses: Mutex<Vec<ToolCallStatus>>,
}

impl ApprovingPort {
    fn take(&self) -> (usize, Vec<ToolCallStatus>) {
        (
            std::mem::take(&mut *self.prompts.lock().unwrap()),
            std::mem::take(&mut *self.statuses.lock().unwrap()),
        )
    }
}

impl AcpClientPort for ApprovingPort {
    fn send_session_notification(
        &self,
        notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        if let SessionUpdate::ToolCallUpdate(update) = notification.update {
            if let Some(status) = update.fields.status {
                self.statuses.lock().unwrap().push(status);
            }
        }
        Box::pin(async { Ok(()) })
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        *self.prompts.lock().unwrap() += 1;
        Box::pin(async {
            Ok(RequestPermissionResponse::new(
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("allow_once")),
            ))
        })
    }
}

fn probe_turns(count: usize) -> Vec<ScriptedTurn> {
    (0..count)
        .flat_map(|index| {
            [
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                    ToolCall {
                        id: format!("call-{index}"),
                        name: PROBE.into(),
                        arguments: serde_json::json!({}),
                    },
                )])),
                ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                    "done".into(),
                )])),
            ]
        })
        .collect()
}

// Covers: after session/set_mode, tool writes must obey the new mode: plan
// denies without asking, supervised asks the host, bypass runs without asking.
// The live conversation and session identity survive every swap.
// Owner: ACP session transaction
#[tokio::test]
async fn set_mode_changes_tool_authorization_and_keeps_the_conversation() {
    let home = tempfile::tempdir().unwrap();
    let config = test_config();
    let cases = [
        (PermissionMode::Plan, 0, ToolCallStatus::Failed),
        (PermissionMode::Supervised, 1, ToolCallStatus::Completed),
        (PermissionMode::Bypass, 0, ToolCallStatus::Completed),
    ];
    let mut tools = AppToolSet::disabled();
    tools.add_bundle(StaticToolBundle::new(vec![Arc::new(WriteProbe)]));
    let startup = crate::app::acp::AcpStartup::for_tests(config.clone(), home.path());
    let mut host = host(home.path(), &config, probe_turns(cases.len()), tools).await;
    let session_id = host.built.session.id().clone();
    let history = host.built.session.history();
    let port = ApprovingPort::default();

    for (mode, host_prompts, status) in cases {
        host.set_permission_mode(mode, &config).await.unwrap();
        host.prompt(
            PromptRequest::new(
                host.acp_session_id.clone(),
                vec![AcpContentBlock::from("write")],
            ),
            &port,
            &startup,
        )
        .await
        .unwrap();
        assert_eq!(port.take(), (host_prompts, vec![status]), "{mode:?}");
        assert_eq!(host.built.session.id(), &session_id, "{mode:?}");
        assert_eq!(
            host.built.session.history()[..history.len()],
            history[..],
            "{mode:?}"
        );
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
    let mut host = host(home.path(), &config, Vec::new(), AppToolSet::disabled()).await;

    let error = host
        .set_permission_mode(PermissionMode::Auto, &config)
        .await
        .expect_err("auto needs a classifier model");

    assert_eq!(error.code, ErrorCode::InvalidParams);
    assert_eq!(host.permission.mode, PermissionMode::Bypass);
    host.shutdown().await;
}

// Covers: hosts that use config options (Zed) switch modes through the `mode`
// option; it must apply like session/set_mode and echo the new mode, and an
// unknown value must be rejected without changing the mode.
// Owner: ACP session transaction
#[tokio::test]
async fn mode_config_option_switches_the_permission_mode() {
    let home = tempfile::tempdir().unwrap();
    let config = test_config();
    let mut host = host(home.path(), &config, Vec::new(), AppToolSet::disabled()).await;
    let session_id = host.acp_session_id.clone();
    let request =
        |value: &str| SetSessionConfigOptionRequest::new(session_id.clone(), "mode", value);

    let response = host
        .set_config_option(request("plan"), &config)
        .await
        .unwrap();
    let mode = response
        .config_options
        .iter()
        .find(|option| option.id.0.as_ref() == "mode")
        .expect("mode option");
    let SessionConfigKind::Select(select) = &mode.kind else {
        panic!("expected a select option, got {:?}", mode.kind);
    };
    assert_eq!(select.current_value.0.as_ref(), "plan");
    assert_eq!(host.permission.mode, PermissionMode::Plan);

    let error = host
        .set_config_option(request("yolo"), &config)
        .await
        .expect_err("unknown mode");
    assert_eq!(error.code, ErrorCode::InvalidParams);
    assert_eq!(host.permission.mode, PermissionMode::Plan);
    host.shutdown().await;
}
