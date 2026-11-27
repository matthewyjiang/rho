//! What a notification card is about, typed: its stable id, how urgently it
//! wants the reader, its subject, and one item per event it tells of. The
//! daemon sends it beside the card's event
//! ([`crate::SessionEventEnvelope::notification`]), so a client can draw
//! chips, fold routine cards and open what a card is about without parsing
//! [`super::NotificationCard`]'s display strings.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::methods::{PullRequestId, SessionId};

/// Identifies one notification card within a session's conversation: the
/// same on every replay of it, live, after a reattach, and after a daemon
/// restart. A fork keeps the ids of the cards it copies.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct NotificationId(String);

impl NotificationId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NotificationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The typed side of one notification card.
///
/// # Next major
///
/// NEXT_MAJOR(rho-protocol): fold NotificationInfo's id, urgency, subject and items into NotificationCard and remove SessionEventEnvelope::notification.
///
/// `NotificationCard` has only public fields and is not
/// `#[non_exhaustive]`, so a field is a major change; until then this rides
/// beside the card's event. Saved cards already store these fields flat
/// beside the card's own, as `NotificationCard` will hold them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct NotificationInfo {
    pub id: NotificationId,
    /// What the card asks of the reader: that of its most urgent item
    /// ([`NotificationItem::urgency`]).
    pub urgency: NotificationUrgency,
    /// What the card as a whole is about.
    pub subject: NotificationSubject,
    /// One per event the card tells of, in the card's order. Rho sends at
    /// least one.
    pub items: Vec<NotificationItem>,
}

impl NotificationInfo {
    /// Builds it from each of its fields.
    pub fn new(
        id: NotificationId,
        urgency: NotificationUrgency,
        subject: NotificationSubject,
        items: Vec<NotificationItem>,
    ) -> Self {
        Self {
            id,
            urgency,
            subject,
            items,
        }
    }
}

/// One event a notification card tells of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct NotificationItem {
    pub outcome: NotificationOutcome,
    /// What this event asks of the reader, by the policy that gives the
    /// card its [`NotificationInfo::urgency`], so a card that bundles a
    /// routine event with an urgent one can tell them apart. A daemon built
    /// before items had it leaves it out, which reads as
    /// [`NotificationUrgency::Unknown`]: show such an item, like one whose
    /// urgency this crate does not know, as urgent as its card.
    #[serde(
        default = "NotificationUrgency::unknown",
        skip_serializing_if = "NotificationUrgency::is_unknown"
    )]
    pub urgency: NotificationUrgency,
    /// What this event is about, which may be narrower than the card's.
    pub subject: NotificationSubject,
    /// The event in words, one line of plain text.
    pub text: String,
}

impl NotificationItem {
    /// Builds it from each of its fields.
    pub fn new(
        outcome: NotificationOutcome,
        urgency: NotificationUrgency,
        subject: NotificationSubject,
        text: impl Into<String>,
    ) -> Self {
        Self {
            outcome,
            urgency,
            subject,
            text: text.into(),
        }
    }
}

/// What a notification card, or one of its items, is about. Protocol minors
/// may add kinds.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotificationSubject {
    /// A pull request, and where to open it.
    PullRequest { id: PullRequestId, url: String },
    /// Another session, such as one this session started.
    Session { id: SessionId },
    /// A subagent run, by its run id, as [`crate::BackgroundItem::Subagent`]
    /// names it.
    Subagent {
        run: String,
        /// The subagent's own session, when the daemon knew it; runtimes
        /// that keep no Rho session never have one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionId>,
    },
    /// A workflow run, by its run id, as [`crate::BackgroundItem::Workflow`]
    /// names it.
    Workflow { run: String },
    /// A background process, by its [`crate::SessionProcess::id`].
    Process { id: String },
    /// Something none of the other kinds names.
    Other,
    /// A kind a newer daemon sends.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// What a notification card, or one of its items, asks of the reader.
/// Protocol minors may add values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotificationUrgency {
    /// The reader should act: a question or approval waits, or something
    /// failed in a way that needs a fix.
    NeedsYou,
    /// Worth a look: work finished, a pull request moved.
    Notable,
    /// Progress, and work stopped before it finished; safe to fold away.
    Routine,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

impl NotificationUrgency {
    /// What an item that names no urgency reads as.
    fn unknown() -> Self {
        Self::Unknown
    }

    /// An `Unknown` item urgency is left off the wire, so it reads back the
    /// same. Daemons never send it.
    fn is_unknown(&self) -> bool {
        *self == Self::Unknown
    }
}

/// What happened in one notification item, for a client's icon or label;
/// [`NotificationItem::text`] says it in words. Protocol minors may add
/// values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotificationOutcome {
    /// Progress from work still under way, such as a subagent's message.
    Update,
    /// Another session waits for an answer to its question.
    Question,
    /// A subagent asks its parent to act before it goes on.
    ActionRequested,
    /// Still running.
    Running,
    /// Finished as it should: a run succeeded, a process exited 0.
    Completed,
    /// Failed: a run's error, a process's non-zero exit.
    Failed,
    /// A process never started.
    FailedToStart,
    /// A process exited without an exit code.
    Exited,
    /// Stopped before it finished: by someone, or, for a subagent, by the
    /// daemon stopping.
    Stopped,
    /// Someone cancelled the turn.
    Cancelled,
    /// It ran past its time limit and was stopped.
    TimedOut,
    /// The daemon stopped while it ran.
    Interrupted,
    /// The model's answer was cut off at its output limit.
    CutOff,
    /// The model refused.
    Refused,
    /// The turn reached its step limit before the model answered.
    OutOfSteps,
    /// A pull request's head moved.
    NewCommits,
    /// A pull request's checks failed, or finished with some failing.
    ChecksFailed,
    /// A pull request's checks all passed.
    ChecksPassed,
    /// Every check of a pull request finished, none failing.
    ChecksFinished,
    /// A pull request has new comments or reviews.
    NewComments,
    /// A pull request's branch conflicts with its base.
    MergeConflict,
    /// A pull request merged.
    Merged,
    /// Closed without merging.
    Closed,
    /// Rho stopped watching a pull request that kept getting comment-only
    /// updates.
    WatchStopped,
    /// Rho stopped watching a pull request it could not read.
    Unreadable,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

#[cfg(test)]
#[path = "notification_info_tests.rs"]
mod tests;
