//! The aside's session: a fork of the followed session in the daemon
//! (`rho/session/fork`), opened at the aside's first prompt.
//!
//! The fork starts from the session's conversation as last saved and never
//! writes back to it. The daemon opens it supervised (plan, when the session
//! was saved in plan) and tells its model it is a side chat that must leave
//! the workspace alone; reads run free. The overlay has no approval prompt,
//! so the aside denies every approval the fork asks for, and keeps a plan
//! in plan mode. Its setup is the session's, less what the overlay cannot
//! show (questions and the `ui` tool), subagents, whose work would outlive
//! a cancelled turn, and idle wake, since nothing would show the turn it
//! starts. The fork is memory-only: the daemon saves nothing of it, and
//! discards it once nothing follows it and it is idle, as when the TUI
//! quits or its connection goes.
//!
//! The worker follows the fork on the session's connection, by its own
//! route, and maps its live events, not the replay of the forked
//! conversation, to [`SideChatEvent`]s. Dropping the handle stops the
//! fork's turn, if one runs, then deletes the fork once the turn ended, as
//! the daemon deletes no session mid-turn.

use std::{collections::HashSet, sync::Arc};

use rho_protocol::{
    methods::{
        SessionAnswer, SessionAttach, SessionCancel, SessionDelete, SessionFork, SessionPrompt,
    },
    Ask, ClientEnvironment, ContentBlock, IdleWake, InputAnswer, InputRequest, SessionAnswerParams,
    SessionAttachParams, SessionCancelParams, SessionDeleteParams, SessionEvent, SessionForkParams,
    SessionId, SessionPart, SessionPromptParams, SessionSetup, Surfaces, ToolCallId, TurnOutcome,
};
use tokio::sync::mpsc;

use crate::{
    app::daemon_client::{Daemon, Part, Routed, StreamPosition},
    tui::approval::ApprovalPrompt,
};

/// Why the aside denies an approval, which the model hears and the overlay
/// shows.
pub(super) const READ_ONLY: &str = "side chats are read-only";

/// What the overlay shows of the fork.
#[derive(Debug)]
pub(super) enum SideChatEvent {
    AssistantDelta(String),
    /// The provider discarded the reply it was streaming.
    AssistantReset,
    ToolStarted(String),
    /// The aside denied an approval the fork asked for; its summary.
    Denied(String),
    /// Something to tell the user that ends no turn, such as a daemon notice.
    Notice(String),
    Finished,
    Failed(String),
    Cancelled,
}

/// What forks the session: its daemon, its id, and its setup.
pub(super) struct ForkLaunch {
    pub(super) daemon: Arc<Daemon>,
    pub(super) parent: SessionId,
    pub(super) setup: SessionSetup,
}

#[derive(Debug)]
enum Command {
    Submit(String),
    Cancel,
}

/// The overlay's end of the worker. Dropping it winds the fork down.
pub(super) struct SideChatHandle {
    commands: mpsc::UnboundedSender<Command>,
    events: mpsc::UnboundedReceiver<SideChatEvent>,
}

impl SideChatHandle {
    pub(super) fn submit(&self, prompt: String) {
        let _ = self.commands.send(Command::Submit(prompt));
    }

    pub(super) fn cancel(&self) {
        let _ = self.commands.send(Command::Cancel);
    }

    pub(super) fn try_recv(&mut self) -> Option<SideChatEvent> {
        self.events.try_recv().ok()
    }
}

/// Starts the worker; the fork opens with the first submit. The overlay
/// submits only while no turn of the aside runs.
pub(super) fn spawn_side_chat(launch: ForkLaunch) -> SideChatHandle {
    let (commands, command_rx) = mpsc::unbounded_channel();
    let (event_tx, events) = mpsc::unbounded_channel();
    tokio::spawn(worker(launch, command_rx, event_tx));
    SideChatHandle { commands, events }
}

/// The fork the worker follows.
struct Fork {
    id: SessionId,
    routed: mpsc::UnboundedReceiver<Routed>,
    position: StreamPosition,
    /// Whether a turn the aside started still runs.
    running: bool,
    /// The tool calls the overlay was told of.
    tools: HashSet<ToolCallId>,
}

async fn worker(
    launch: ForkLaunch,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<SideChatEvent>,
) {
    let ForkLaunch {
        daemon,
        parent,
        setup,
    } = launch;
    let setup = fork_setup(setup);
    let mut fork: Option<Fork> = None;
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None => break,
                Some(Command::Cancel) => {
                    if let Some(fork) = fork.as_ref().filter(|fork| fork.running) {
                        cancel(&daemon, &fork.id).await;
                    }
                }
                Some(Command::Submit(prompt)) => {
                    let open = match fork.as_mut() {
                        Some(open) => open,
                        None => match open(&daemon, &parent, &setup).await {
                            Ok(opened) => fork.insert(opened),
                            Err(error) => {
                                let _ = events.send(SideChatEvent::Failed(format!(
                                    "could not start side chat: {error:#}"
                                )));
                                continue;
                            }
                        },
                    };
                    match send_prompt(&daemon, &open.id, prompt).await {
                        Ok(()) => open.running = true,
                        Err(error) => {
                            let _ = events.send(SideChatEvent::Failed(format!(
                                "could not send side chat message: {error:#}"
                            )));
                        }
                    }
                }
            },
            routed = next_routed(fork.as_mut()) => match routed {
                Some(Routed::Event { seq, event }) => {
                    let Some(open) = fork.as_mut() else {
                        continue;
                    };
                    if open.position.observe(seq) == Part::Live {
                        apply(&daemon, open, event, &events).await;
                    }
                }
                // The daemon ended the attachment, or the connection went,
                // and the memory-only fork with it: the next prompt forks
                // the session again.
                Some(Routed::Detached(_) | Routed::Lost) | None => {
                    if let Some(lost) = fork.take() {
                        daemon.unroute(&lost.id);
                        let _ = events.send(if lost.running {
                            SideChatEvent::Failed(
                                "could not complete side chat: lost its session in the rho daemon"
                                    .into(),
                            )
                        } else {
                            SideChatEvent::Notice(
                                "the side chat lost its session in the rho daemon; the next \
                                 message starts it over"
                                    .into(),
                            )
                        });
                    }
                }
            },
        }
    }
    if let Some(fork) = fork {
        close(&daemon, fork).await;
    }
}

async fn next_routed(fork: Option<&mut Fork>) -> Option<Routed> {
    match fork {
        Some(fork) => fork.routed.recv().await,
        None => std::future::pending().await,
    }
}

/// The setup a fork attaches with: the session's, without what the overlay
/// cannot show or what would run unseen.
fn fork_setup(mut setup: SessionSetup) -> SessionSetup {
    setup.without.insert(SessionPart::Questions);
    setup.without.insert(SessionPart::Subagents);
    // Only a new session takes it; the fork opens with it.
    setup.without.remove(&SessionPart::Saving);
    setup.surfaces = Surfaces::default();
    setup.idle_wake = IdleWake::default();
    setup
}

/// Forks `parent` memory-only and attaches to the fork with `setup`.
async fn open(daemon: &Daemon, parent: &SessionId, setup: &SessionSetup) -> anyhow::Result<Fork> {
    let mut memory_only = setup.clone();
    memory_only.without.insert(SessionPart::Saving);
    let forked = daemon
        .request::<SessionFork>(&SessionForkParams {
            session: parent.clone(),
            environment: ClientEnvironment::of_this_process(),
            setup: memory_only,
        })
        .await?;
    let id = forked.session;
    let routed = daemon.route_channel(&id);
    let attached = daemon
        .request::<SessionAttach>(&SessionAttachParams {
            session: id.clone(),
            after: None,
            environment: ClientEnvironment::of_this_process(),
            setup: setup.clone(),
        })
        .await;
    match attached {
        Ok(response) => Ok(Fork {
            position: StreamPosition::attached(&response),
            id,
            routed,
            running: false,
            tools: HashSet::new(),
        }),
        Err(error) => {
            daemon.unroute(&id);
            delete(daemon, &id).await;
            Err(error.into())
        }
    }
}

async fn send_prompt(daemon: &Daemon, id: &SessionId, prompt: String) -> anyhow::Result<()> {
    daemon
        .request::<SessionPrompt>(&SessionPromptParams {
            session: id.clone(),
            content: vec![ContentBlock::Text { text: prompt }],
            timeout_ms: None,
            environment: ClientEnvironment::of_this_process(),
            grant: None,
            idempotency_key: None,
            author: Default::default(),
        })
        .await?;
    Ok(())
}

async fn cancel(daemon: &Daemon, id: &SessionId) {
    let _ = daemon
        .request::<SessionCancel>(&SessionCancelParams {
            session: id.clone(),
        })
        .await;
}

/// Deletes the fork now. One the delete misses is memory-only all the same:
/// the daemon discards it once it is idle and nothing follows it.
async fn delete(daemon: &Daemon, id: &SessionId) {
    if let Err(error) = daemon
        .request::<SessionDelete>(&SessionDeleteParams {
            session: id.clone(),
            unless_attached: false,
            work: rho_protocol::DeleteWork::Refuse,
        })
        .await
    {
        tracing::debug!(%error, session = %id, "could not delete side chat fork");
    }
}

/// Shows one live event of the fork, and answers what it asks.
async fn apply(
    daemon: &Daemon,
    fork: &mut Fork,
    event: SessionEvent,
    events: &mpsc::UnboundedSender<SideChatEvent>,
) {
    let shown = match event {
        SessionEvent::AgentMessageChunk { text, .. } => SideChatEvent::AssistantDelta(text),
        SessionEvent::ResponseDiscarded => SideChatEvent::AssistantReset,
        SessionEvent::ToolCall(update) => {
            if !fork.tools.insert(update.id.clone()) {
                return;
            }
            SideChatEvent::ToolStarted(update.title.unwrap_or_else(|| "tool".into()))
        }
        SessionEvent::Notice { text } => SideChatEvent::Notice(text),
        SessionEvent::InputRequested { request } => {
            match refuse(daemon, &fork.id, *request).await {
                Some(denied) => denied,
                None => return,
            }
        }
        SessionEvent::TurnEnded { outcome, .. } => {
            fork.running = false;
            match outcome {
                TurnOutcome::Answered { .. } | TurnOutcome::MaxSteps { .. } => {
                    SideChatEvent::Finished
                }
                TurnOutcome::Cancelled => SideChatEvent::Cancelled,
                TurnOutcome::Failed { error } => SideChatEvent::Failed(format!(
                    "could not complete side chat: {}",
                    error.message
                )),
                TurnOutcome::TimedOut { .. } => {
                    SideChatEvent::Failed("could not complete side chat: it timed out".into())
                }
                // `Unknown`, or an outcome a newer daemon sends.
                _ => SideChatEvent::Finished,
            }
        }
        // The overlay shows the reply and its tools alone: no reasoning,
        // progress, cards, or session state.
        SessionEvent::State { .. }
        | SessionEvent::UserMessage { .. }
        | SessionEvent::ThoughtChunk { .. }
        | SessionEvent::Plan { .. }
        | SessionEvent::Usage { .. }
        | SessionEvent::TitleChanged { .. }
        | SessionEvent::MarkChanged { .. }
        | SessionEvent::Compaction { .. }
        | SessionEvent::ProviderRequest
        | SessionEvent::ProviderRetry { .. }
        | SessionEvent::Card { .. }
        | SessionEvent::InputResolved { .. }
        | SessionEvent::InputHeld { .. }
        | SessionEvent::OptionsChanged { .. }
        | SessionEvent::SteeringDelivered { .. }
        | SessionEvent::CacheWarm { .. }
        | SessionEvent::Background { .. }
        | SessionEvent::Activity { .. }
        | SessionEvent::Agent { .. }
        | SessionEvent::PullRequests { .. } => return,
        // `Unknown`, or an event a newer daemon sends.
        _ => return,
    };
    let _ = events.send(shown);
}

/// Answers an input request of the fork as a read-only aside does: an
/// approval is denied, and a plan stays a plan. Returns what the overlay
/// shows of a denied approval.
async fn refuse(daemon: &Daemon, id: &SessionId, request: InputRequest) -> Option<SideChatEvent> {
    let (answer, shown) = match &request.ask {
        Ask::Approval(ask) => (
            InputAnswer::Deny {
                reason: Some(READ_ONLY.into()),
            },
            Some(SideChatEvent::Denied(
                ApprovalPrompt::of_daemon_ask(ask).summary().join(" · "),
            )),
        ),
        Ask::PlanApproval(_) => (
            InputAnswer::KeepPlanning {
                feedback: Some(READ_ONLY.into()),
            },
            None,
        ),
        // The fork opens without questions; one asked all the same goes
        // unanswered by the person.
        Ask::Question(_) => (InputAnswer::Dismiss, None),
        // `Unknown`, or an ask a newer daemon sends: the turn waits on it
        // until the person cancels it.
        _ => return None,
    };
    // A request that closed meanwhile needs no answer.
    let _ = daemon
        .request::<SessionAnswer>(&SessionAnswerParams {
            session: id.clone(),
            request: request.id,
            answer,
        })
        .await;
    shown
}

/// Stops the fork's turn, if one runs, then deletes the fork once it ended.
async fn close(daemon: &Daemon, mut fork: Fork) {
    if fork.running {
        cancel(daemon, &fork.id).await;
        while let Some(routed) = fork.routed.recv().await {
            match routed {
                Routed::Event {
                    event: SessionEvent::TurnEnded { .. },
                    ..
                } => break,
                Routed::Event { .. } => {}
                Routed::Detached(_) | Routed::Lost => break,
            }
        }
    }
    daemon.unroute(&fork.id);
    delete(daemon, &fork.id).await;
}
