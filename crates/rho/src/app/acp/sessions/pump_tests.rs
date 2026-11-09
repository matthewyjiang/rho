use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{atomic::AtomicU64, Arc, Mutex},
};

use agent_client_protocol::{
    schema::v1::{
        CompleteElicitationNotification, CreateElicitationRequest, CreateElicitationResponse,
        RequestPermissionRequest, RequestPermissionResponse, SessionId as AcpSessionId,
        SessionNotification, SessionUpdate,
    },
    Error as AcpError,
};
use pretty_assertions::assert_eq;
use rho_protocol::{
    self as wire, AnswerStop, Cursor, DetachReason, JobState, Replay, Seq, SessionAttachResponse,
    SessionEvent, SessionId, SessionSetup, TurnOutcome,
};
use tokio::sync::{mpsc, oneshot, watch};

use super::spawn;
use crate::{
    app::{
        acp::{
            daemon,
            host_features::HostFeatures,
            inputs::InputContext,
            sessions::{
                Accept, Attacher, Attachment, Control, Inbound, Opening, Shared, Show, Waiter,
            },
            AcpClientPort,
        },
        daemon_client::{test_support::attach_response, Daemon, Routed},
    },
    herdr::HerdrReporter,
};

const SESSION: &str = "s1";

/// A host that records the updates it is sent.
#[derive(Default)]
struct Host {
    sent: Mutex<Vec<SessionNotification>>,
}

impl Host {
    /// The ids of the user messages the host was shown.
    fn echoed(&self) -> Vec<String> {
        let sent = self.sent.lock().unwrap();
        sent.iter()
            .filter_map(|notification| match &notification.update {
                SessionUpdate::UserMessageChunk(chunk) => {
                    chunk.message_id.as_ref().map(|id| id.0.to_string())
                }
                _ => None,
            })
            .collect()
    }
}

impl AcpClientPort for Host {
    fn send_session_notification(
        &self,
        notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        self.sent.lock().unwrap().push(notification);
        Box::pin(std::future::ready(Ok(())))
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        Box::pin(std::future::ready(Err(AcpError::internal_error())))
    }

    fn create_elicitation(
        &self,
        _request: CreateElicitationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<CreateElicitationResponse, AcpError>> + Send + '_>>
    {
        Box::pin(std::future::ready(Err(AcpError::internal_error())))
    }

    fn complete_elicitation(
        &self,
        _notification: CompleteElicitationNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        Box::pin(std::future::ready(Ok(())))
    }
}

/// A daemon that answers attaches from a script, recording the cursor each
/// resumed after.
#[derive(Default)]
struct Script {
    answers: Mutex<VecDeque<Result<SessionAttachResponse, AcpError>>>,
    after: Mutex<Vec<Option<Seq>>>,
}

impl Attacher for Script {
    fn attach<'a>(
        &'a self,
        _session: &'a SessionId,
        after: Option<&'a Cursor>,
        _setup: &'a SessionSetup,
    ) -> Pin<Box<dyn Future<Output = Result<SessionAttachResponse, AcpError>> + Send + 'a>> {
        self.after
            .lock()
            .unwrap()
            .push(after.map(|cursor| cursor.seq));
        let answer = self
            .answers
            .lock()
            .unwrap()
            .pop_front()
            .expect("an attach the test did not script");
        Box::pin(std::future::ready(answer))
    }
}

fn user(id: &str) -> SessionEvent {
    SessionEvent::UserMessage {
        message: wire::MessageId::new(id),
        content: vec![wire::ContentBlock::Text { text: id.into() }],
        steer: None,
    }
}

fn answered(text: &str) -> TurnOutcome {
    TurnOutcome::Answered {
        text: text.into(),
        stop: AnswerStop::EndTurn,
    }
}

fn ended(outcome: TurnOutcome) -> SessionEvent {
    SessionEvent::TurnEnded { outcome }
}

/// A prompt of this host's, armed on the pump.
struct Prompt {
    outcome: oneshot::Receiver<Result<TurnOutcome, AcpError>>,
    accepted: Arc<Mutex<Option<String>>>,
}

impl Prompt {
    /// How the prompt ended, or `None` while it waits.
    fn outcome(&mut self) -> Option<Result<TurnOutcome, AcpError>> {
        self.outcome.try_recv().ok()
    }

    fn accepted(&self) -> Option<String> {
        self.accepted.lock().unwrap().clone()
    }
}

/// One pump, started on an opening attach with nothing to replay.
struct Pump {
    inbound: mpsc::UnboundedSender<Inbound>,
    shared: Arc<Shared>,
    host: Arc<Host>,
    script: Arc<Script>,
    seq: u64,
}

impl Pump {
    fn start(state: JobState, script: Script) -> Self {
        let host = Arc::new(Host::default());
        let script = Arc::new(script);
        let response = attach_response(Replay::Reset, 0, state);
        let shared = Arc::new(Shared {
            options: Mutex::new(response.options.clone()),
            attachment: watch::channel(Attachment::Stale).0,
        });
        let port: Arc<dyn AcpClientPort> = host.clone();
        let context = InputContext {
            daemon: Arc::new(Daemon::default()),
            session: SessionId::new(SESSION),
            acp_id: AcpSessionId::new(SESSION),
            port,
            features: HostFeatures::default(),
            placeholders: Arc::new(AtomicU64::new(0)),
        };
        let (inbound, receiver) = mpsc::unbounded_channel();
        let (replayed, _) = oneshot::channel();
        let (gate, gate_receiver) = oneshot::channel();
        let _ = gate.send(());
        let attacher: Arc<dyn Attacher> = script.clone();
        spawn(
            context,
            attacher,
            receiver,
            Arc::clone(&shared),
            SessionSetup::default(),
            &HerdrReporter::from_env_vars(|_| None),
            Opening {
                response,
                show: Show::Skip,
                replayed,
                gate: gate_receiver,
            },
        );
        Self {
            inbound,
            shared,
            host,
            script,
            seq: 0,
        }
    }

    fn routed(&self, routed: Routed) {
        self.inbound.send(Inbound::Routed(routed)).unwrap();
    }

    /// A live event, numbered after the last.
    fn live(&mut self, event: SessionEvent) {
        self.seq += 1;
        self.event(self.seq, event);
    }

    fn event(&self, seq: u64, event: SessionEvent) {
        self.routed(Routed::Event {
            seq: Seq(seq),
            event,
        });
    }

    fn arm(&self) -> Prompt {
        let (outcome, receiver) = oneshot::channel();
        let accepted = Arc::new(Mutex::new(None));
        let heard = Arc::clone(&accepted);
        let accept: Accept = Box::new(move |id| *heard.lock().unwrap() = Some(id.0.to_string()));
        let waiter = Waiter {
            outcome,
            accept: Arc::new(Mutex::new(Some(accept))),
        };
        self.inbound
            .send(Inbound::Control(Control::Arm(waiter)))
            .unwrap();
        Prompt {
            outcome: receiver,
            accepted,
        }
    }

    /// Returns once the pump handled everything sent before.
    async fn flush(&self) {
        let (done, flushed) = oneshot::channel();
        self.inbound
            .send(Inbound::Control(Control::Flush(done)))
            .unwrap();
        flushed.await.unwrap();
    }

    fn attachment(&self) -> Attachment {
        *self.shared.attachment.borrow()
    }
}

/// What a test drives a pump with.
enum Step {
    Arm,
    Live(SessionEvent),
}

/// What a prompt and its host saw.
#[derive(Debug, PartialEq)]
struct Seen {
    outcome: Option<Result<TurnOutcome, AcpError>>,
    accepted: Option<String>,
    echoed: Vec<String>,
}

// Covers: the first user message after arming is the prompt's (accepted,
// not echoed) and the first turn end after it ends the prompt; a turn end
// before it, such as a cancelled turn's, does not; other clients' messages
// are echoed.
// Owner: ACP session pump
#[tokio::test]
async fn armed_prompts_match_their_own_turn() {
    use Step::{Arm, Live};
    let cases = [
        (
            "own turn",
            vec![Arm, Live(user("m1")), Live(ended(answered("a")))],
            Seen {
                outcome: Some(Ok(answered("a"))),
                accepted: Some("m1".into()),
                echoed: vec![],
            },
        ),
        (
            "an earlier turn's end is not the prompt's",
            vec![
                Arm,
                Live(ended(TurnOutcome::Cancelled)),
                Live(user("m1")),
                Live(ended(answered("a"))),
            ],
            Seen {
                outcome: Some(Ok(answered("a"))),
                accepted: Some("m1".into()),
                echoed: vec![],
            },
        ),
        (
            "another client's prompt before arming",
            vec![
                Live(user("m0")),
                Arm,
                Live(user("m1")),
                Live(ended(answered("a"))),
            ],
            Seen {
                outcome: Some(Ok(answered("a"))),
                accepted: Some("m1".into()),
                echoed: vec!["m0".into()],
            },
        ),
        (
            "turn still running",
            vec![Arm, Live(user("m1"))],
            Seen {
                outcome: None,
                accepted: Some("m1".into()),
                echoed: vec![],
            },
        ),
    ];
    for (label, steps, expected) in cases {
        let mut pump = Pump::start(JobState::Idle, Script::default());
        let mut prompt = None;
        for step in steps {
            match step {
                Arm => prompt = Some(pump.arm()),
                Live(event) => pump.live(event),
            }
        }
        pump.flush().await;
        let mut prompt = prompt.expect("every row arms");
        let seen = Seen {
            outcome: prompt.outcome(),
            accepted: prompt.accepted(),
            echoed: pump.host.echoed(),
        };
        assert_eq!(seen, expected, "{label}");
    }
}

// Covers: an attachment that fell behind is resumed after the cursor at
// once; a tail's missed events act as live, and a reset's skipped replay
// settles the armed prompt by how it ends: its turn ended with the job
// idle, or its user message began the running turn (C9).
// Owner: ACP session pump
#[tokio::test]
async fn lagged_attachments_resume_and_settle_the_prompt() {
    struct Case {
        label: &'static str,
        /// Live events before the lag, after arming.
        before: Vec<SessionEvent>,
        resumed: SessionAttachResponse,
        replay: Vec<(u64, SessionEvent)>,
        /// Live events after the replay.
        after: Vec<SessionEvent>,
        resumed_after: Option<Seq>,
        expected: Seen,
    }
    let cases = [
        Case {
            label: "tail holds the turn's end",
            before: vec![user("m1")],
            resumed: attach_response(Replay::Tail, 1, JobState::Completed),
            replay: vec![(2, ended(answered("a")))],
            after: vec![],
            resumed_after: Some(Seq(1)),
            expected: Seen {
                outcome: Some(Ok(answered("a"))),
                accepted: Some("m1".into()),
                echoed: vec![],
            },
        },
        Case {
            label: "reset after the prompt's turn ended",
            before: vec![user("m1")],
            resumed: attach_response(Replay::Reset, 4, JobState::Completed),
            replay: vec![
                (0, user("m0")),
                (0, ended(answered("old"))),
                (0, user("m1")),
                (2, ended(answered("a"))),
            ],
            after: vec![],
            resumed_after: Some(Seq(1)),
            expected: Seen {
                outcome: Some(Ok(answered("a"))),
                accepted: Some("m1".into()),
                echoed: vec![],
            },
        },
        Case {
            label: "reset while the prompt's turn runs",
            before: vec![],
            resumed: attach_response(Replay::Reset, 3, JobState::Running),
            replay: vec![
                (0, user("m0")),
                (0, ended(answered("old"))),
                (1, user("m1")),
            ],
            after: vec![ended(answered("a"))],
            resumed_after: None,
            expected: Seen {
                outcome: Some(Ok(answered("a"))),
                accepted: Some("m1".into()),
                echoed: vec![],
            },
        },
        Case {
            label: "reset before the prompt's turn began",
            before: vec![],
            resumed: attach_response(Replay::Reset, 1, JobState::Idle),
            replay: vec![(0, user("m0"))],
            after: vec![user("m1")],
            resumed_after: None,
            expected: Seen {
                outcome: None,
                accepted: Some("m1".into()),
                echoed: vec![],
            },
        },
    ];
    for case in cases {
        let script = Script::default();
        script.answers.lock().unwrap().push_back(Ok(case.resumed));
        let mut pump = Pump::start(JobState::Running, script);
        let mut prompt = pump.arm();
        for event in case.before {
            pump.live(event);
        }
        pump.routed(Routed::Detached(DetachReason::Lagged));
        for (seq, event) in case.replay {
            pump.event(seq, event);
        }
        pump.seq = 10;
        for event in case.after {
            pump.live(event);
        }
        pump.flush().await;
        let seen = Seen {
            outcome: prompt.outcome(),
            accepted: prompt.accepted(),
            echoed: pump.host.echoed(),
        };
        assert_eq!(seen, case.expected, "{}", case.label);
        assert_eq!(
            *pump.script.after.lock().unwrap(),
            vec![case.resumed_after],
            "{}",
            case.label
        );
    }
}

// Covers: an attachment that ends fails the armed prompt, marks the
// session stale (attach again) or deleted, releases whoever waits for the
// running job to finish (a delete, C4), drops the ended attachment's later
// events, and refuses prompts armed after it.
// Owner: ACP session pump
#[tokio::test]
async fn ended_attachments_fail_the_prompt_and_release_job_waits() {
    let resume_failed = AcpError::internal_error().data("attach refused");
    let cases = [
        (
            "connection lost",
            Routed::Lost,
            None,
            daemon::lost(),
            Attachment::Stale,
        ),
        (
            "daemon stopping",
            Routed::Detached(DetachReason::DaemonStopping),
            None,
            AcpError::internal_error().data("rho daemon is stopping"),
            Attachment::Stale,
        ),
        (
            "daemon restarting",
            Routed::Detached(DetachReason::Restarting),
            None,
            AcpError::internal_error().data("rho daemon restarted"),
            Attachment::Stale,
        ),
        (
            "deleted",
            Routed::Detached(DetachReason::Deleted),
            None,
            AcpError::resource_not_found(Some(SESSION.to_owned())),
            Attachment::Deleted,
        ),
        (
            "lagged and the reattach failed",
            Routed::Detached(DetachReason::Lagged),
            Some(Err(resume_failed.clone())),
            resume_failed,
            Attachment::Stale,
        ),
    ];
    for (label, ending, answer, error, attachment) in cases {
        let script = Script::default();
        script.answers.lock().unwrap().extend(answer);
        let mut pump = Pump::start(JobState::Running, script);
        let mut prompt = pump.arm();
        // The pump marks the session attached before it reads anything.
        pump.flush().await;
        assert!(pump.attachment().busy(), "{label}");
        pump.routed(ending);
        pump.live(user("m9"));
        let mut late = pump.arm();
        pump.flush().await;
        assert_eq!(
            (
                prompt.outcome(),
                late.outcome(),
                // What a delete waiting for the job reads: no longer busy.
                pump.attachment(),
                pump.host.echoed(),
            ),
            (
                Some(Err(error)),
                Some(Err(daemon::lost())),
                attachment,
                vec![],
            ),
            "{label}"
        );
    }
}
