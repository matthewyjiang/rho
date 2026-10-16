use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use agent_client_protocol::{
    schema::v1::{
        CancelNotification, ContentBlock as AcpContentBlock, PromptRequest,
        RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
        SelectedPermissionOutcome, SessionConfigKind, SessionModeId, SessionNotification,
        SessionUpdate, SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason,
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
    app::{
        acp::{agent::RhoAcpAgent, AcpClientPort, AcpStartup},
        session_assembly::BuiltSession,
    },
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
        super::super::persist::HostStorage::saved(stored),
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

/// Tool that holds its turn open until released or cancelled, and reports
/// when it starts so a test can act while the prompt is running.
struct HoldTurn {
    started: tokio::sync::mpsc::UnboundedSender<()>,
    release: Arc<tokio::sync::Notify>,
}

const HOLD: &str = "hold_turn";

impl Tool for HoldTurn {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: HOLD.into(),
            description: "test tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        let started = self.started.clone();
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            let _ = started.send(());
            tokio::select! {
                biased;
                () = context.cancellation().cancelled() => Err(ToolError::cancelled()),
                () = release.notified() => Ok(ToolOutput::text("released")),
            }
        })
    }
}

/// The two ACP requests that switch a session's permission mode.
#[derive(Clone, Copy, Debug)]
enum ModeRequest {
    SetMode,
    ConfigOption,
}

impl ModeRequest {
    async fn send(
        self,
        agent: &Arc<RhoAcpAgent>,
        session_id: &SessionId,
        mode: &str,
    ) -> Result<(), ErrorCode> {
        let result = match self {
            Self::SetMode => agent
                .set_session_mode(SetSessionModeRequest::new(
                    session_id.clone(),
                    SessionModeId::new(mode),
                ))
                .await
                .map(drop),
            Self::ConfigOption => agent
                .set_config_option(SetSessionConfigOptionRequest::new(
                    session_id.clone(),
                    "mode",
                    mode,
                ))
                .await
                .map(drop),
        };
        result.map_err(|error| error.code)
    }
}

// Covers: a mode change sent while a prompt runs, or while a cancelled prompt
// winds down (T3 Code cancels, reconfigures, then prompts), must not fail as
// busy. It waits for the turn to end, then applies to the next prompt.
// Owner: ACP agent session map
#[tokio::test]
async fn mode_change_during_a_turn_waits_and_applies_to_the_next_prompt() {
    let cases = [
        (ModeRequest::SetMode, /*cancel*/ true),
        (ModeRequest::ConfigOption, /*cancel*/ true),
        (ModeRequest::SetMode, /*cancel*/ false),
        (ModeRequest::ConfigOption, /*cancel*/ false),
    ];
    for (request, cancel) in cases {
        let label = format!("{request:?}, cancel {cancel}");
        let home = tempfile::tempdir().unwrap();
        let config = test_config();
        let (started_tx, mut started) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Notify::new());
        let mut tools = AppToolSet::disabled();
        tools.add_bundle(StaticToolBundle::new(vec![
            Arc::new(HoldTurn {
                started: started_tx,
                release: Arc::clone(&release),
            }),
            Arc::new(WriteProbe),
        ]));
        let mut turns = vec![ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::ToolCall(ToolCall {
                id: "hold".into(),
                name: HOLD.into(),
                arguments: serde_json::json!({}),
            }),
        ]))];
        // A released tool hands its result back to the model; a cancelled
        // turn ends without another request.
        if !cancel {
            turns.push(ScriptedTurn::completed(ModelResponse::Assistant(vec![
                ContentBlock::Text("held".into()),
            ])));
        }
        turns.extend(probe_turns(1));
        let mut host = host(home.path(), &config, turns, tools).await;
        // Supervised asks the host before the probe writes, so the later
        // switch to plan is observable. This also binds the tools.
        host.set_permission_mode(PermissionMode::Supervised, &config)
            .await
            .unwrap();
        let session_id = host.acp_session_id.clone();
        let agent = Arc::new(RhoAcpAgent::new(AcpStartup::for_tests(
            config.clone(),
            home.path(),
        )));
        agent.install_for_tests(session_id.clone(), host).await;
        let port = Arc::new(ApprovingPort::default());
        let prompt = |text: &str| PromptRequest::new(session_id.clone(), vec![text.into()]);

        let turn = tokio::spawn({
            let agent = Arc::clone(&agent);
            let port = Arc::clone(&port);
            let request = prompt("hold");
            async move { agent.prompt(request, &*port).await }
        });
        started.recv().await.expect("the turn reaches its tool");
        if cancel {
            agent
                .cancel(CancelNotification::new(session_id.clone()))
                .await;
        }
        let change = request.send(&agent, &session_id, "plan");
        tokio::pin!(change);
        assert!(
            futures_util::poll!(&mut change).is_pending(),
            "{label}: the change must wait for the running turn"
        );
        if !cancel {
            release.notify_one();
        }
        let stop = turn
            .await
            .expect("prompt task")
            .expect("turn ends")
            .stop_reason;
        let expected_stop = if cancel {
            StopReason::Cancelled
        } else {
            StopReason::EndTurn
        };
        assert_eq!((stop, change.await), (expected_stop, Ok(())), "{label}");

        port.take();
        agent.prompt(prompt("write"), &*port).await.unwrap();
        assert_eq!(
            port.take(),
            (0, vec![ToolCallStatus::Failed]),
            "{label}: plan must deny the next prompt's write without asking"
        );
        agent.shutdown_all().await;
    }
}

// Covers: a mode change that reaches the host only after a replacement claimed
// it must fail and leave that host's mode alone, not report success for a
// host the replacement is about to discard.
// Owner: ACP agent session map
#[tokio::test]
async fn mode_change_reaching_a_replaced_host_fails_without_applying() {
    for request in [ModeRequest::SetMode, ModeRequest::ConfigOption] {
        let home = tempfile::tempdir().unwrap();
        let config = test_config();
        let mut tools = AppToolSet::disabled();
        tools.add_bundle(StaticToolBundle::new(vec![Arc::new(WriteProbe)]));
        let mut host = host(home.path(), &config, probe_turns(1), tools).await;
        host.set_permission_mode(PermissionMode::Supervised, &config)
            .await
            .unwrap();
        let session_id = host.acp_session_id.clone();
        let agent = Arc::new(RhoAcpAgent::new(AcpStartup::for_tests(
            config.clone(),
            home.path(),
        )));
        agent.install_for_tests(session_id.clone(), host).await;
        let (held, replaced) = agent.hold_host_for_tests(&session_id).await;

        let change = request.send(&agent, &session_id, "plan");
        tokio::pin!(change);
        assert!(futures_util::poll!(&mut change).is_pending(), "{request:?}");
        // `publish` marks the old session replaced before it queues for the
        // host, so the change gets the host first and must see the flag.
        replaced.store(true, std::sync::atomic::Ordering::Release);
        drop(held);
        assert_eq!(change.await, Err(ErrorCode::InvalidRequest), "{request:?}");

        // As after a failed load, the session stays published; it must still
        // be supervised, so the write asks the host.
        replaced.store(false, std::sync::atomic::Ordering::Release);
        let port = ApprovingPort::default();
        agent
            .prompt(
                PromptRequest::new(session_id.clone(), vec!["write".into()]),
                &port,
            )
            .await
            .unwrap();
        assert_eq!(
            port.take(),
            (1, vec![ToolCallStatus::Completed]),
            "{request:?}"
        );
        agent.shutdown_all().await;
    }
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
