//! What a session hears of the sessions it started through session control
//! (the `sessions` tool's `new` and `prompt`): a turn it began in one of
//! them ended, or stopped to wait for a person's input. [`PeerInbox`] holds
//! those notices for the starter until a turn boundary delivers them, as
//! finished background work is delivered: at the next boundary of a turn
//! under way, or by the turn the daemon starts on the idle session.
//!
//! The inbox keeps one notice per started session, the latest, so a
//! session that waited for input and then ended tells only of its end, and
//! one delivery drains every notice. It also counts the turns that still
//! owe a notice ([`Expectation`]), which keep the starter loaded and
//! active while they run.
//!
//! Nothing here is saved: a daemon restart forgets notices not yet
//! delivered and the turns that owed one.

use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use rho_protocol::{
    display::{
        NotificationCard, NotificationDelivery, NotificationPreview, NotificationTone,
        NotificationVisibility,
    },
    InputRequestId,
};
use tokio::sync::Notify;

use crate::tools::{
    agent::{push_excerpt, MODEL_NOTIFICATION_BYTES, RESULT_EXCERPT_BYTES},
    notification_format::join_budgeted_sections,
};

const NOTIFICATION_HEADER: &str = "[session notification]\n\nSessions you started:\n";
const NOTIFICATION_FOOTER: &str = "\n\nRead a session's whole transcript with `sessions read`. rho tells you when a turn you started in a session ends or waits for a person, so there is no need to poll with `sessions wait`; if a `sessions wait` already returned this, nothing is new.\n\nThis is an automated notification, not a user message.";

/// One starter session's notices of the sessions it started. Cheap to
/// clone; clones share the notices.
#[derive(Clone, Default)]
pub(crate) struct PeerInbox {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Told when a notice arrives or is put back.
    arrived: Arc<Notify>,
}

#[derive(Default)]
struct State {
    /// The latest notice of each started session, by its id.
    notices: HashMap<String, Queued>,
    /// The started sessions whose turn owes a notice, each with the token
    /// of its [`Expectation`].
    expected: HashMap<String, u64>,
    /// The next notice's seq, and the next expectation's token.
    next: u64,
}

/// A notice with its arrival order.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Queued {
    seq: u64,
    notice: PeerNotice,
}

/// What a started session's turn did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PeerNotice {
    /// The started session's id.
    pub(crate) session: String,
    /// Its title, once it has one.
    pub(crate) title: Option<String>,
    pub(crate) event: PeerEvent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PeerEvent {
    /// The turn ended.
    TurnEnded(PeerTurnEnd),
    /// The turn waits for a person to answer input request `request`,
    /// which `title` names.
    WaitingForInput {
        request: InputRequestId,
        title: String,
    },
}

/// How a started session's turn ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PeerTurnEnd {
    /// It completed with this final answer.
    Answered {
        answer: String,
    },
    /// It completed at its step limit, with this last reply.
    StepLimit {
        reply: String,
    },
    Failed {
        error: String,
    },
    Cancelled,
    /// The daemon cancelled it at the limit its prompt set.
    TimedOut {
        timeout_ms: u64,
    },
    /// The daemon stopped before it ended.
    Interrupted,
}

/// The notices one turn boundary took, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PeerBatch {
    notices: Vec<Queued>,
}

/// A started session's turn that owes its starter a notice. Dropping it
/// unsent forgets the turn, as when the started session goes before its
/// turn ends.
pub(crate) struct Expectation {
    inbox: PeerInbox,
    session: String,
    token: u64,
}

impl PeerInbox {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // Each change inserts or removes whole entries, so a panic cannot
        // leave one half-changed.
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Records that a turn of started session `session` owes a notice,
    /// replacing any turn of it that still did.
    pub(crate) fn expect(&self, session: &str) -> Expectation {
        let mut state = self.state();
        let token = state.next;
        state.next += 1;
        state.expected.insert(session.to_owned(), token);
        Expectation {
            inbox: self.clone(),
            session: session.to_owned(),
            token,
        }
    }

    /// Whether a notice waits that no turn has taken.
    pub(crate) fn has_notices(&self) -> bool {
        !self.state().notices.is_empty()
    }

    /// Whether a started session's turn still owes a notice.
    pub(crate) fn expecting(&self) -> bool {
        !self.state().expected.is_empty()
    }

    /// Whether a notice waits or could still arrive.
    pub(crate) fn outstanding(&self) -> bool {
        let state = self.state();
        !state.notices.is_empty() || !state.expected.is_empty()
    }

    /// Completes when a notice arrives or is put back. Subscribe before
    /// checking [`Self::has_notices`], so a notice in between is not
    /// missed.
    pub(crate) fn arrived_owned(&self) -> impl Future<Output = ()> + Send + 'static {
        Arc::clone(&self.shared.arrived).notified_owned()
    }

    /// Takes every notice that waits, oldest first.
    pub(crate) fn take(&self) -> PeerBatch {
        let mut notices: Vec<Queued> = self
            .state()
            .notices
            .drain()
            .map(|(_, queued)| queued)
            .collect();
        notices.sort_by_key(|queued| queued.seq);
        PeerBatch { notices }
    }

    /// Puts back notices a turn took and did not deliver, unless a newer
    /// one of the same session arrived since. An empty batch returns at
    /// once, without telling waiters: a turn drops one on every pass, and
    /// telling would spin it.
    pub(crate) fn restore(&self, batch: &PeerBatch) {
        if batch.is_empty() {
            return;
        }
        {
            let mut state = self.state();
            for queued in &batch.notices {
                state
                    .notices
                    .entry(queued.notice.session.clone())
                    .or_insert_with(|| queued.clone());
            }
        }
        self.shared.arrived.notify_waiters();
    }

    /// Queues `notice`, replacing the one of its session that waits, and,
    /// under the same lock, forgets the expectation `ends` names, if any.
    fn queue(&self, notice: PeerNotice, ends: Option<u64>) {
        {
            let mut state = self.state();
            let seq = state.next;
            state.next += 1;
            if let Some(token) = ends {
                forget(&mut state, &notice.session, token);
            }
            state
                .notices
                .insert(notice.session.clone(), Queued { seq, notice });
        }
        self.shared.arrived.notify_waiters();
    }
}

/// Forgets `session`'s expectation if it is still the one `token` names.
fn forget(state: &mut State, session: &str, token: u64) {
    if state.expected.get(session) == Some(&token) {
        state.expected.remove(session);
    }
}

impl Expectation {
    /// Tells the starter that the turn waits for input; it still owes the
    /// notice of its end.
    pub(crate) fn tell_waiting(&self, title: Option<String>, request: InputRequestId, ask: String) {
        self.inbox.queue(
            PeerNotice {
                session: self.session.clone(),
                title,
                event: PeerEvent::WaitingForInput {
                    request,
                    title: ask,
                },
            },
            /*ends*/ None,
        );
    }

    /// Tells the starter that the turn ended as `end`.
    pub(crate) fn tell_ended(self, title: Option<String>, end: PeerTurnEnd) {
        self.inbox.queue(
            PeerNotice {
                session: self.session.clone(),
                title,
                event: PeerEvent::TurnEnded(end),
            },
            Some(self.token),
        );
    }
}

impl Drop for Expectation {
    fn drop(&mut self) {
        forget(&mut self.inbox.state(), &self.session, self.token);
    }
}

impl PeerBatch {
    pub(crate) fn is_empty(&self) -> bool {
        self.notices.is_empty()
    }

    fn notices(&self) -> impl Iterator<Item = &PeerNotice> {
        self.notices.iter().map(|queued| &queued.notice)
    }

    /// The batch's notices, oldest first.
    #[cfg(test)]
    pub(crate) fn notices_for_tests(&self) -> Vec<PeerNotice> {
        self.notices().cloned().collect()
    }

    /// The model's text for the batch, within [`MODEL_NOTIFICATION_BYTES`]:
    /// each session's state first, then the final answers and errors,
    /// each cut to [`RESULT_EXCERPT_BYTES`], with what room is left, as
    /// subagent notifications are (`tools/agent_output.rs`).
    pub(crate) fn prompt(&self) -> String {
        let body_bytes = MODEL_NOTIFICATION_BYTES
            .saturating_sub(NOTIFICATION_HEADER.len() + NOTIFICATION_FOOTER.len());
        let mut body = join_budgeted_sections(
            self.notices().map(PeerNotice::summary),
            "\n\n",
            body_bytes,
            |remaining| {
                format!("... {remaining} session section(s) omitted; use `sessions list` or `sessions wait`")
            },
        );
        let texts: Vec<(&str, &str)> = self
            .notices()
            .filter_map(|notice| Some((notice.session.as_str(), notice.text()?)))
            .collect();
        if !texts.is_empty() {
            let heading = "\n\nFinal answers and errors:";
            if body.len() + heading.len() > body_bytes {
                return format!("{NOTIFICATION_HEADER}{body}{NOTIFICATION_FOOTER}");
            }
            body.push_str(heading);
        }
        for (session, text) in texts {
            let label = format!("\n\nsession {session}:\n");
            if body.len() + label.len() >= body_bytes {
                break;
            }
            body.push_str(&label);
            let available = (body_bytes - body.len()).min(RESULT_EXCERPT_BYTES);
            push_excerpt(&mut body, text, available);
        }
        format!("{NOTIFICATION_HEADER}{body}{NOTIFICATION_FOOTER}")
    }

    /// One transcript card per notice, in the batch's order.
    pub(crate) fn cards(&self) -> Vec<NotificationCard> {
        self.notices().map(PeerNotice::card).collect()
    }
}

impl PeerNotice {
    /// The session's lines of the model's text: who it is, its state as
    /// `sessions wait` names it, and how its turn ended or what it waits
    /// for.
    fn summary(&self) -> String {
        let name = match &self.title {
            Some(title) => format!("session {} ({title})", self.session),
            None => format!("session {}", self.session),
        };
        let outcome = match &self.event {
            PeerEvent::TurnEnded(end) => match end {
                PeerTurnEnd::Answered { .. } => "outcome: answered".to_owned(),
                PeerTurnEnd::StepLimit { .. } => {
                    "outcome: ran out of steps; its last reply is below".to_owned()
                }
                PeerTurnEnd::Failed { .. } => "outcome: failed; the error is below".to_owned(),
                PeerTurnEnd::Cancelled => "outcome: cancelled".to_owned(),
                PeerTurnEnd::TimedOut { timeout_ms } => {
                    format!("outcome: timed out after {timeout_ms} ms")
                }
                PeerTurnEnd::Interrupted => {
                    "outcome: interrupted; the daemon stopped before the turn ended".to_owned()
                }
            },
            PeerEvent::WaitingForInput { request, title } => format!(
                "waits for a person to answer input request {request}: {title}. `sessions wait` returns the whole question."
            ),
        };
        format!("{name}: {}\n{outcome}", self.state())
    }

    /// The session's state, as `sessions wait` names it.
    fn state(&self) -> &'static str {
        match &self.event {
            PeerEvent::TurnEnded(end) => match end {
                PeerTurnEnd::Answered { .. } | PeerTurnEnd::StepLimit { .. } => "completed",
                PeerTurnEnd::Failed { .. } => "failed",
                PeerTurnEnd::Cancelled => "cancelled",
                PeerTurnEnd::TimedOut { .. } => "timed_out",
                PeerTurnEnd::Interrupted => "interrupted",
            },
            PeerEvent::WaitingForInput { .. } => "waiting_for_input",
        }
    }

    /// The final answer, last reply or error the turn ended with, if any.
    fn text(&self) -> Option<&str> {
        match &self.event {
            PeerEvent::TurnEnded(
                PeerTurnEnd::Answered { answer: text }
                | PeerTurnEnd::StepLimit { reply: text }
                | PeerTurnEnd::Failed { error: text },
            ) => Some(text.as_str()).filter(|text| !text.is_empty()),
            PeerEvent::TurnEnded(
                PeerTurnEnd::Cancelled | PeerTurnEnd::TimedOut { .. } | PeerTurnEnd::Interrupted,
            )
            | PeerEvent::WaitingForInput { .. } => None,
        }
    }

    /// The notice's transcript card, drawn as a subagent's result card is.
    fn card(&self) -> NotificationCard {
        use NotificationPreview::{Full, Truncated};
        use NotificationTone::{Accent, Error, Success, Warning};
        let (label, tone, preview, body) = match &self.event {
            PeerEvent::TurnEnded(end) => match end {
                PeerTurnEnd::Answered { answer } => {
                    ("Completed", Success, Truncated, answer.clone())
                }
                PeerTurnEnd::StepLimit { reply } => {
                    ("Out of steps", Warning, Truncated, reply.clone())
                }
                PeerTurnEnd::Failed { error } => ("Failed", Error, Full, error.clone()),
                PeerTurnEnd::Cancelled => ("Cancelled", Accent, Truncated, String::new()),
                PeerTurnEnd::TimedOut { timeout_ms } => (
                    "Timed out",
                    Error,
                    Truncated,
                    format!("the turn timed out after {timeout_ms} ms"),
                ),
                PeerTurnEnd::Interrupted => (
                    "Interrupted",
                    Accent,
                    Truncated,
                    "the daemon stopped before the turn ended".to_owned(),
                ),
            },
            PeerEvent::WaitingForInput { request, title } => (
                "Waiting for input",
                Warning,
                Full,
                format!("input request {request}: {title}"),
            ),
        };
        let name = self.title.as_deref().unwrap_or(&self.session);
        NotificationCard {
            title: format!("{label} · {name}"),
            sender: self.session.clone(),
            recipient: "parent".into(),
            delivery: NotificationDelivery::Received,
            tone,
            preview,
            visibility: NotificationVisibility::Conversation,
            reference: Some(self.session.clone()),
            subtitle: None,
            body,
            output: None,
            details: vec![format!("session: {}", self.session)],
        }
    }
}

#[cfg(test)]
#[path = "peer_sessions_tests.rs"]
mod tests;
