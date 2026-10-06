//! Deterministic ACP peer shared by Channel tests and the debug-only
//! `rho __acp-fixture-agent` subcommand. Blocking client requests run in
//! spawned foreground-independent tasks; callbacks never await an outgoing
//! request on the dispatch loop.

use agent_client_protocol::{
    schema::{
        v1::{
            AuthenticateRequest, AuthenticateResponse, CancelNotification, InitializeRequest,
            InitializeResponse, NewSessionRequest, NewSessionResponse, PermissionOption,
            PromptRequest, PromptResponse, RequestPermissionRequest, SessionMode, SessionModeState,
            SessionNotification, SessionUpdate, SetSessionModeRequest, SetSessionModeResponse,
            StopReason, ToolCallUpdate,
        },
        ProtocolVersion,
    },
    Agent, Client, ConnectTo, ConnectionTo, Error, Responder, UntypedMessage,
};
use rho_tools::cancellation::RunCancellation;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    fs::File,
    io::Write,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct Script {
    #[serde(default)]
    pub(crate) initialize_error: Option<String>,
    #[serde(default)]
    pub(crate) authenticate_error: Option<String>,
    #[serde(default)]
    pub(crate) new_session_error: Option<String>,
    #[serde(default)]
    pub(crate) set_mode_error: Option<String>,
    #[serde(default)]
    pub(crate) modes: Vec<String>,
    pub(crate) turns: Vec<Vec<Step>>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub(crate) enum Step {
    Update {
        update: Box<SessionUpdate>,
    },
    Permission {
        call: Box<ToolCallUpdate>,
        options: Vec<PermissionOption>,
    },
    Extension {
        method: String,
        params: Value,
    },
    Stop {
        reason: StopReason,
    },
    /// Signal readiness, then await a real session/cancel notification.
    Hang,
    /// Close the transport while the prompt is still pending.
    Disconnect,
}

/// Whole wire requests/replies, without assertions embedded in the fake.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Record {
    pub(crate) requests: Vec<Value>,
    pub(crate) replies: Vec<Value>,
    /// Each entry is also appended here as one JSON line. The debug fixture
    /// needs this because Rho terminates the agent rather than letting it exit.
    #[serde(skip)]
    journal: Option<File>,
}

impl Record {
    fn push_request(&mut self, request: Value) {
        self.write_journal(&json!({"request": request}));
        self.requests.push(request);
    }

    fn push_reply(&mut self, reply: Value) {
        self.write_journal(&json!({"reply": reply}));
        self.replies.push(reply);
    }

    fn write_journal(&mut self, entry: &Value) {
        if let Some(journal) = self.journal.as_mut() {
            writeln!(journal, "{entry}").expect("write scripted agent journal");
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    Prompt,
    Waiting,
    Cancel,
}

pub(crate) struct ScriptedAgent {
    script: Script,
    record: Arc<Mutex<Record>>,
    signals: mpsc::UnboundedSender<Signal>,
}

impl ScriptedAgent {
    pub(crate) fn new(
        script: Script,
    ) -> (Self, Arc<Mutex<Record>>, mpsc::UnboundedReceiver<Signal>) {
        let record = Arc::new(Mutex::new(Record::default()));
        let (signals, receiver) = mpsc::unbounded_channel();
        (
            Self {
                script,
                record: Arc::clone(&record),
                signals,
            },
            record,
            receiver,
        )
    }

    /// Also append every recorded request and reply to `journal` as JSONL.
    #[cfg(debug_assertions)]
    pub(crate) fn with_journal(self, journal: File) -> Self {
        self.record.lock().expect("script record").journal = Some(journal);
        self
    }

    pub(crate) async fn run(self, transport: impl ConnectTo<Agent> + 'static) -> Result<(), Error> {
        let Self {
            script,
            record,
            signals,
        } = self;
        let closed = RunCancellation::new();
        let cancelled = RunCancellation::new();
        let initialize_record = Arc::clone(&record);
        let initialize_error = script.initialize_error;
        let authenticate_error = script.authenticate_error;
        let authenticate_record = Arc::clone(&record);
        let new_record = Arc::clone(&record);
        let new_error = script.new_session_error;
        let modes = script.modes;
        let mode_record = Arc::clone(&record);
        let mode_error = script.set_mode_error;
        let prompt_record = Arc::clone(&record);
        let turns = Arc::new(Mutex::new(VecDeque::from(script.turns)));
        let prompt_closed = closed.clone();
        let prompt_cancelled = cancelled.clone();
        let prompt_signals = signals.clone();
        Agent
            .builder()
            .name("rho-scripted-agent")
            .on_receive_request(
                async move |request: InitializeRequest, responder, _cx| {
                    record_request(&initialize_record, "initialize", &request);
                    match &initialize_error {
                        Some(message) => responder.respond_with_error(fake_error(message)),
                        None => responder.respond(InitializeResponse::new(ProtocolVersion::V1)),
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: AuthenticateRequest, responder, _cx| {
                    record_request(&authenticate_record, "authenticate", &request);
                    match &authenticate_error {
                        Some(message) => responder.respond_with_error(fake_error(message)),
                        None => responder.respond(AuthenticateResponse::new()),
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: NewSessionRequest, responder, _cx| {
                    record_request(&new_record, "session/new", &request);
                    match &new_error {
                        Some(message) => responder.respond_with_error(fake_error(message)),
                        None => responder.respond(
                            NewSessionResponse::new("scripted-session").modes(
                                SessionModeState::new(
                                    "agent",
                                    modes
                                        .iter()
                                        .map(|mode| SessionMode::new(mode.clone(), mode.clone()))
                                        .collect(),
                                ),
                            ),
                        ),
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: SetSessionModeRequest, responder, _cx| {
                    record_request(&mode_record, "session/set_mode", &request);
                    match &mode_error {
                        Some(message) => responder.respond_with_error(fake_error(message)),
                        None => responder.respond(SetSessionModeResponse::new()),
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: PromptRequest, responder, cx| {
                    record_request(&prompt_record, "session/prompt", &request);
                    let _ = prompt_signals.send(Signal::Prompt);
                    let steps = turns
                        .lock()
                        .expect("script turns")
                        .pop_front()
                        .ok_or_else(|| fake_error("unexpected prompt"))?;
                    let record = Arc::clone(&prompt_record);
                    let signals = prompt_signals.clone();
                    let cancelled = prompt_cancelled.clone();
                    let closed = prompt_closed.clone();
                    cx.clone().spawn(async move {
                        play_turn(
                            cx,
                            request,
                            responder,
                            steps,
                            TurnControls {
                                record,
                                signals,
                                cancelled,
                                closed,
                            },
                        )
                        .await
                    })
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_notification(
                async move |request: CancelNotification, _cx| {
                    record_request(&record, "session/cancel", &request);
                    let _ = signals.send(Signal::Cancel);
                    cancelled.cancel();
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .connect_with(transport, async move |cx| {
                tokio::select! { _ = cx.incoming_closed() => {}, _ = closed.cancelled() => {} }
                Ok(())
            })
            .await
    }
}

fn record_request(record: &Mutex<Record>, method: &str, request: &impl Serialize) {
    record
        .lock()
        .expect("script record")
        .push_request(json!({"method": method, "params": request}));
}

fn fake_error(message: &str) -> Error {
    let mut error = Error::internal_error();
    error.message = message.into();
    error
}

struct TurnControls {
    record: Arc<Mutex<Record>>,
    signals: mpsc::UnboundedSender<Signal>,
    cancelled: RunCancellation,
    closed: RunCancellation,
}

async fn play_turn(
    cx: ConnectionTo<Client>,
    request: PromptRequest,
    responder: Responder<PromptResponse>,
    steps: Vec<Step>,
    controls: TurnControls,
) -> Result<(), Error> {
    let TurnControls {
        record,
        signals,
        cancelled,
        closed,
    } = controls;
    for step in steps {
        match step {
            Step::Update { update } => cx.send_notification(SessionNotification::new(
                request.session_id.clone(),
                *update,
            ))?,
            Step::Permission { call, options } => {
                let response = cx
                    .send_request(RequestPermissionRequest::new(
                        request.session_id.clone(),
                        *call,
                        options,
                    ))
                    .block_task()
                    .await?;
                record.lock().expect("script record").push_reply(
                    json!({"method": "session/request_permission", "result": response}),
                );
            }
            Step::Extension { method, params } => {
                let response = cx
                    .send_request(UntypedMessage::new(&method, params)?)
                    .block_task()
                    .await;
                let reply = match response {
                    Ok(result) => json!({"method": method, "result": result}),
                    Err(error) => json!({"method": method, "error": error}),
                };
                record.lock().expect("script record").push_reply(reply);
            }
            Step::Stop { reason } => return responder.respond(PromptResponse::new(reason)),
            Step::Hang => {
                let _ = signals.send(Signal::Waiting);
                cancelled.cancelled().await;
                return responder.respond(PromptResponse::new(StopReason::Cancelled));
            }
            Step::Disconnect => {
                closed.cancel();
                return std::future::pending().await;
            }
        }
    }
    Err(fake_error("script turn has no stop step"))
}
