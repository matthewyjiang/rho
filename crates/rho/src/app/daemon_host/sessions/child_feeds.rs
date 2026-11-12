//! The event streams of the Rho subagent and workflow agent node runs the
//! daemon's sessions start (#1668), so a client follows a run as it
//! follows a session: by observing the session the run writes.
//!
//! A run's stream is a [`Feed`] like a loaded session's, with one turn: the
//! run. It opens when the run opens its session, which may be long after
//! the run started, while it waits for capacity; an observe meanwhile waits
//! with it. The stream carries the run's events as its child sends them,
//! the run's job states, and how the run ended, then closes. Nobody
//! attaches to it to prompt, steer, or answer: a run's questions go to its
//! parent session.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use rho_protocol::{
    self as wire, Cursor, DetachReason, SessionAttachResponse, Subscription, TurnOutcome,
};
use rho_sdk::SessionId;
use tokio::sync::watch;

use super::{attach_response, feed::Feed};
use crate::app::daemon_host::jobs::JobState;

/// The streams of the runs under way, by the session each writes.
#[derive(Clone, Default)]
pub(in crate::app::daemon_host) struct ChildFeeds {
    runs: Arc<Mutex<HashMap<SessionId, Listed>>>,
}

/// One run's stream as an observe finds it.
#[derive(Clone)]
struct Listed {
    workspace: PathBuf,
    /// The run's feed, once the run opened its session.
    feed: watch::Receiver<Option<Arc<Feed>>>,
}

impl ChildFeeds {
    /// Lists the stream of a run about to write `session` in `workspace`,
    /// whose job is in `state`. The stream is listed until the returned
    /// [`ChildStream`] drops.
    pub(in crate::app::daemon_host) fn list(
        &self,
        session: SessionId,
        workspace: PathBuf,
        state: &JobState,
    ) -> ChildStream {
        let (feed, listed) = watch::channel(None);
        self.runs().insert(
            session.clone(),
            Listed {
                workspace,
                feed: listed,
            },
        );
        ChildStream {
            feeds: self.clone(),
            session,
            turn: wire::TurnId::new(uuid::Uuid::new_v4().hyphenated().to_string()),
            state: state.into(),
            feed,
        }
    }

    /// Subscribes a client to the stream of the run writing `session`,
    /// resuming after its cursor when the stream still can, once the run
    /// has opened the session. `None` when no run under way writes
    /// `session`, or the run ended before it opened it. A run that gave way
    /// to another of the same session meanwhile, as a workflow node's next
    /// attempt, is followed through that one.
    pub(super) async fn attach(
        &self,
        session: &SessionId,
        after: Option<&Cursor>,
    ) -> Option<(SessionAttachResponse, Subscription)> {
        loop {
            let Listed {
                workspace,
                mut feed,
            } = self.runs().get(session)?.clone();
            let opened = feed
                .wait_for(Option::is_some)
                .await
                .ok()
                .map(|feed| feed.clone());
            // A run that ended unopened took its listing with it; the next
            // lookup finds a later run's, or none.
            if let Some(Some(feed)) = opened {
                return Some(attach_response(&workspace, feed.attach(after)));
            }
        }
    }

    fn runs(&self) -> MutexGuard<'_, HashMap<SessionId, Listed>> {
        // Entries are only inserted and removed whole.
        self.runs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One run's stream, as the task that follows the run writes it. Dropping
/// it closes the stream and takes it off the list.
pub(in crate::app::daemon_host) struct ChildStream {
    feeds: ChildFeeds,
    session: SessionId,
    /// The stream's one turn: the run.
    turn: wire::TurnId,
    /// Where the run's job is, which the feed opens in.
    state: wire::JobState,
    feed: watch::Sender<Option<Arc<Feed>>>,
}

impl ChildStream {
    /// Opens the stream: the run opened its session, which runs with
    /// `options`.
    pub(in crate::app::daemon_host) fn open(&self, options: wire::SessionOptions) {
        self.feed.send_if_modified(|feed| {
            if feed.is_some() {
                return false;
            }
            *feed = Some(Arc::new(Feed::new(
                self.state,
                Some(self.turn.clone()),
                /*title*/ None,
                options,
            )));
            true
        });
    }

    /// Sends `events` to the clients following the run. A run sends none
    /// before it opens its session.
    pub(in crate::app::daemon_host) fn publish(
        &self,
        events: impl IntoIterator<Item = wire::SessionEvent>,
    ) {
        if let Some(feed) = self.feed.borrow().as_ref() {
            feed.publish(events);
        }
    }

    /// Tells the clients following the run that its job is now in `state`.
    pub(in crate::app::daemon_host) fn moved(&mut self, state: &JobState) {
        self.state = state.into();
        self.publish([self.state_event()]);
    }

    /// Ends the run's turn as `outcome`, if it has one to tell, with its
    /// job in `state`, in one publish, as a session's turn ends.
    pub(in crate::app::daemon_host) fn end(
        &mut self,
        outcome: Option<TurnOutcome>,
        state: &JobState,
    ) {
        self.state = state.into();
        let ended = outcome.map(|outcome| wire::SessionEvent::TurnEnded {
            turn: self.turn.clone(),
            outcome,
        });
        self.publish(ended.into_iter().chain([self.state_event()]));
    }

    /// Detaches the clients following the run with `reason`, instead of
    /// the [`DetachReason::NotLoaded`] a dropped stream closes with.
    pub(in crate::app::daemon_host) fn close(&self, reason: DetachReason) {
        if let Some(feed) = self.feed.borrow().as_ref() {
            feed.close(reason);
        }
    }

    fn state_event(&self) -> wire::SessionEvent {
        wire::SessionEvent::State {
            state: self.state,
            turn: Some(self.turn.clone()),
        }
    }
}

impl Drop for ChildStream {
    fn drop(&mut self) {
        // The run's session is saved and no longer written: an observe
        // reads it from its file from here on.
        self.close(DetachReason::NotLoaded);
        let mut runs = self.feeds.runs();
        // A workflow node that runs again writes the same session, and may
        // have listed its stream already.
        if runs
            .get(&self.session)
            .is_some_and(|listed| listed.feed.same_channel(&self.feed.subscribe()))
        {
            runs.remove(&self.session);
        }
    }
}
