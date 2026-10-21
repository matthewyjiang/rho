//! Questions put to the user over ACP: form elicitation for tools that ask,
//! and the plan approval prompt that leaves plan mode.

use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use agent_client_protocol::{
    schema::v1::{
        ContentBlock as AcpContentBlock, CreateElicitationRequest, CreateElicitationResponse,
        ElicitationAcceptAction, ElicitationAction, ElicitationContentValue, PromptRequest,
        RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
        SelectedPermissionOutcome, SessionId, SessionModeId, SessionNotification, SessionUpdate,
        StopReason,
    },
    Error as AcpError,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall, ToolResult, ToolSpec},
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    tool::{Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolOutput},
    HostChoice, HostInputRequest, HostQuestion, SelectionMode, SessionOptions,
};

use super::{mode::SessionPermission, PromptGate, SessionHost};
use crate::app::acp::host_features::{FormDialect, Forms};
use crate::{
    app::{acp::AcpStartup, session_assembly::BuiltSession},
    config::Config,
    diagnostics::RuntimeDiagnostics,
    permission::PermissionMode,
    session::Session as StoredSession,
    tools::{
        plan_exit::PlanExitHost,
        sdk_registry::{AppToolSet, StaticToolBundle},
    },
};

const ASK: &str = "ask_color";

/// Asks one question through the run's host input and reports the answer.
struct AskColor;

impl Tool for AskColor {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: ASK.into(),
            description: "test tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    fn call<'a>(&'a self, _invocation: ToolInvocation, context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let question = HostQuestion::new(
                "color",
                "Which color?",
                vec![
                    HostChoice::new("red", "Red"),
                    HostChoice::new("blue", "Blue"),
                ],
                SelectionMode::One,
            )
            .expect("valid question");
            let request =
                HostInputRequest::questionnaire("Pick", vec![question]).expect("valid request");
            let response =
                context
                    .request_host_input(request)
                    .await
                    .map_err(|error| match error {
                        rho_sdk::Error::Cancelled => ToolError::cancelled(),
                        error => ToolError::new(ToolErrorKind::Execution, error.to_string()),
                    })?;
            Ok(ToolOutput::text(response.answers()["color"].join(",")))
        })
    }
}

/// How the fake host answers `elicitation/create`.
enum FormReply {
    Respond(ElicitationAction),
    /// Never answers; reports when it is asked and when the request is
    /// withdrawn (its future dropped).
    Hold {
        asked: tokio::sync::mpsc::UnboundedSender<()>,
        withdrawn: Arc<AtomicBool>,
    },
}

/// Host that answers questions from a script, picks `plan_answer` at every
/// plan approval, and records every session update.
struct AskingPort {
    form: FormReply,
    plan_answer: &'static str,
    /// Cancels this prompt at the run's final usage update, sent after the
    /// run settled: the second after the plan prompt, which follows the
    /// proposing step's start.
    cancel_at_final_usage: Option<Arc<PromptGate>>,
    usage_after_plan_prompt: Mutex<usize>,
    updates: Mutex<Vec<SessionUpdate>>,
    plan_prompts: Mutex<usize>,
}

impl AskingPort {
    fn new(form: FormReply) -> Self {
        Self {
            form,
            plan_answer: "keep_planning",
            cancel_at_final_usage: None,
            usage_after_plan_prompt: Mutex::default(),
            updates: Mutex::default(),
            plan_prompts: Mutex::default(),
        }
    }

    fn mode_updates(&self) -> Vec<SessionModeId> {
        self.updates
            .lock()
            .unwrap()
            .iter()
            .filter_map(|update| match update {
                SessionUpdate::CurrentModeUpdate(update) => Some(update.current_mode_id.clone()),
                _ => None,
            })
            .collect()
    }
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl super::AcpClientPort for AskingPort {
    fn send_session_notification(
        &self,
        notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        if let (Some(gate), SessionUpdate::UsageUpdate(_)) =
            (&self.cancel_at_final_usage, &notification.update)
        {
            if *self.plan_prompts.lock().unwrap() > 0 {
                let mut seen = self.usage_after_plan_prompt.lock().unwrap();
                *seen += 1;
                if *seen == 2 {
                    gate.cancel();
                }
            }
        }
        self.updates.lock().unwrap().push(notification.update);
        Box::pin(async { Ok(()) })
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        *self.plan_prompts.lock().unwrap() += 1;
        let answer = self.plan_answer;
        Box::pin(async move {
            Ok(RequestPermissionResponse::new(
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(answer)),
            ))
        })
    }

    fn create_elicitation(
        &self,
        _request: CreateElicitationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<CreateElicitationResponse, AcpError>> + Send + '_>>
    {
        match &self.form {
            FormReply::Respond(action) => {
                let response = CreateElicitationResponse::new(action.clone());
                Box::pin(async move { Ok(response) })
            }
            FormReply::Hold { asked, withdrawn } => {
                let _ = asked.send(());
                let flag = DropFlag(Arc::clone(withdrawn));
                Box::pin(async move {
                    let _flag = flag;
                    std::future::pending().await
                })
            }
        }
    }
}

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        },
    )]))
}

fn text(text: &str) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
        text.into(),
    )]))
}

/// A session host on `turns`, in `mode`, for a host that does or does not
/// draw forms.
const STANDARD_FORMS: Forms = Forms::Available(FormDialect::Standard);

async fn host(
    home: &std::path::Path,
    mode: PermissionMode,
    forms: Forms,
    turns: Vec<ScriptedTurn>,
) -> (SessionHost, AcpStartup, Arc<ScriptedProvider>) {
    let config = Config {
        provider: "test".into(),
        model: "model".into(),
        permission_mode: mode,
        ..Config::default()
    };
    let scripted = Arc::new(ScriptedProvider::new(
        ModelIdentity::new("test", "test", "model"),
        turns,
    ));
    let provider: Arc<dyn ModelProvider> = scripted.clone();
    let mut tools = AppToolSet::disabled();
    tools.add_bundle(StaticToolBundle::new(vec![Arc::new(AskColor)]));
    let tools = tools.with_plan_exit_host(&config, PlanExitHost::Permission);
    let runtime = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .unwrap();
    let stored = StoredSession::create_in_root(home, home).unwrap();
    let session = runtime
        .session(SessionOptions::new().id(rho_sdk::SessionId::from_string(stored.id()).unwrap()))
        .await
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
        diagnostics: RuntimeDiagnostics::new(&config),
    };
    let mut startup = AcpStartup::for_tests(config.clone(), home);
    startup.host_features.forms = forms;
    // Switching into `mode` builds the runtime with the tools, the way
    // `session/set_mode` does, so the host starts in another mode.
    let initial = if mode == PermissionMode::Plan {
        PermissionMode::Bypass
    } else {
        PermissionMode::Plan
    };
    let mut host = SessionHost::from_built(
        crate::app::acp::client_io::ClientIo::local(SessionId::new(stored.id())),
        built,
        super::persist::HostStorage::saved(stored),
        Default::default(),
        SessionPermission {
            mode: initial,
            writes: Default::default(),
            workspace_root: home.to_path_buf(),
        },
        &startup,
        Vec::new(),
    );
    host.set_permission_mode(mode, &config).await.unwrap();
    (host, startup, scripted)
}

fn prompt(host: &SessionHost, text: &str) -> PromptRequest {
    PromptRequest::new(
        host.acp_session_id.clone(),
        vec![AcpContentBlock::from(text)],
    )
}

/// The result the model received for tool call `id`.
fn tool_result(provider: &ScriptedProvider, id: &str) -> Option<ToolResult> {
    provider
        .recorded_requests()
        .last()?
        .messages
        .iter()
        .find_map(|message| match message {
            Message::ToolResult(result) if result.id == id => Some(result.clone()),
            _ => None,
        })
}

fn accept(content: &[(&str, &str)]) -> ElicitationAction {
    ElicitationAction::Accept(
        ElicitationAcceptAction::new().content(
            content
                .iter()
                .map(|(key, value)| ((*key).to_owned(), ElicitationContentValue::from(*value)))
                .collect::<std::collections::BTreeMap<_, _>>(),
        ),
    )
}

// Covers: a tool's question reaches a form-capable host as elicitation; the
// accepted answer reaches the tool, and decline or cancel fail only that call
// so the model carries on. A host without forms refuses without asking.
// Owner: ACP session host questions
#[tokio::test]
async fn tool_questions_follow_the_hosts_answer() {
    let cases = [
        (
            "accept",
            STANDARD_FORMS,
            accept(&[("color", "blue")]),
            ToolResult {
                id: "ask".into(),
                ok: true,
                content: "blue".into(),
            },
        ),
        (
            "decline",
            STANDARD_FORMS,
            ElicitationAction::Decline,
            ToolResult {
                id: "ask".into(),
                ok: false,
                content: "the user declined to answer".into(),
            },
        ),
        (
            "cancel",
            STANDARD_FORMS,
            ElicitationAction::Cancel,
            ToolResult {
                id: "ask".into(),
                ok: false,
                content: "the user dismissed the question without answering".into(),
            },
        ),
        (
            "no forms",
            Forms::Unavailable,
            accept(&[("color", "blue")]),
            ToolResult {
                id: "ask".into(),
                ok: false,
                content: "the user declined to answer".into(),
            },
        ),
    ];
    for (label, forms, action, expected) in cases {
        let home = tempfile::tempdir().unwrap();
        let turns = vec![call("ask", ASK, serde_json::json!({})), text("done")];
        let (mut host, startup, provider) =
            host(home.path(), PermissionMode::Bypass, forms, turns).await;
        let port = AskingPort::new(FormReply::Respond(action));

        let response = host
            .prompt(prompt(&host, "ask"), &port, &startup)
            .await
            .unwrap();

        assert_eq!(response.stop_reason, StopReason::EndTurn, "{label}");
        assert_eq!(tool_result(&provider, "ask"), Some(expected), "{label}");
        host.shutdown().await;
    }
}

// Covers: cancelling the prompt while a question is open withdraws the form
// from the host and ends the prompt cancelled instead of waiting on the user.
// Owner: ACP session host questions
#[tokio::test]
async fn cancelling_the_prompt_withdraws_an_open_question() {
    let home = tempfile::tempdir().unwrap();
    let turns = vec![call("ask", ASK, serde_json::json!({}))];
    let (mut host, startup, _provider) =
        host(home.path(), PermissionMode::Bypass, STANDARD_FORMS, turns).await;
    let (asked_tx, mut asked) = tokio::sync::mpsc::unbounded_channel();
    let withdrawn = Arc::new(AtomicBool::new(false));
    let port = AskingPort::new(FormReply::Hold {
        asked: asked_tx,
        withdrawn: Arc::clone(&withdrawn),
    });
    let gate = host.cancel_handle();
    let request = prompt(&host, "ask");

    let (response, ()) = tokio::join!(host.prompt(request, &port, &startup), async {
        asked.recv().await.expect("the host is asked");
        gate.cancel();
    });

    assert_eq!(
        (
            response.unwrap().stop_reason,
            withdrawn.load(Ordering::Acquire)
        ),
        (StopReason::Cancelled, true)
    );
    host.shutdown().await;
}

// Covers: keeping planning at the approval prompt must leave the session in
// plan mode and end the prompt without an implementation turn. Approval is
// covered over stdio in `acp_server_cli`.
// Owner: ACP plan exit
#[tokio::test]
async fn keeping_planning_stays_in_plan_mode_without_another_turn() {
    let home = tempfile::tempdir().unwrap();
    let turns = vec![
        call(
            "plan",
            "exit_plan_mode",
            serde_json::json!({"plan": "1. do it"}),
        ),
        text("proposed"),
        text("implemented"),
    ];
    let (mut host, startup, provider) =
        host(home.path(), PermissionMode::Plan, Forms::Unavailable, turns).await;
    let port = AskingPort::new(FormReply::Respond(ElicitationAction::Decline));

    let response = host
        .prompt(prompt(&host, "plan it"), &port, &startup)
        .await
        .unwrap();

    assert_eq!(
        (
            response.stop_reason,
            *port.plan_prompts.lock().unwrap(),
            host.permission.mode,
            port.mode_updates(),
            provider.recorded_requests().len(),
        ),
        (StopReason::EndTurn, 1, PermissionMode::Plan, Vec::new(), 2)
    );
    host.shutdown().await;
}

// Covers: a cancel that lands after the proposing turn settled but before the
// plan handoff (here, during its final usage update) discards the approval:
// the session stays in plan mode, no mode update is sent, the implementation
// turn never runs, and the prompt ends cancelled
// Owner: ACP plan exit
#[tokio::test]
async fn a_cancel_before_the_handoff_keeps_plan_mode() {
    let home = tempfile::tempdir().unwrap();
    let turns = vec![
        call(
            "plan",
            "exit_plan_mode",
            serde_json::json!({"plan": "1. do it"}),
        ),
        text("proposed"),
        text("implemented"),
    ];
    let (mut host, startup, provider) =
        host(home.path(), PermissionMode::Plan, Forms::Unavailable, turns).await;
    let mut port = AskingPort::new(FormReply::Respond(ElicitationAction::Decline));
    port.plan_answer = "allow_edits";
    port.cancel_at_final_usage = Some(host.cancel_handle());

    let response = host
        .prompt(prompt(&host, "plan it"), &port, &startup)
        .await
        .unwrap();

    assert_eq!(
        (
            response.stop_reason,
            host.permission.mode,
            port.mode_updates(),
            provider.recorded_requests().len(),
        ),
        (StopReason::Cancelled, PermissionMode::Plan, Vec::new(), 2)
    );
    host.shutdown().await;
}
