//! One watch's loop: follow the session, POST each piece of news the watch
//! has not reported, and end once the session settled and that was
//! delivered.
//!
//! The loop is level-triggered. A follow's first news is where the session
//! is when it is done already, so news that came while nobody followed
//! (the server was down, the follow lagged) is delivered when the watch
//! follows again. News equal to the watch's last report ([`NewsKey`]: its
//! state, turn, and open requests), as a reopen's replay or the request
//! the `watch` call itself returned, is not sent again. What a piece of
//! news is to a watch, [`News`], is decided once, for the loop and the
//! `watch` call alike. A follow the daemon keeps dropping before it read
//! anything past its replay stalls the watch after
//! [`MAX_REOPENS_WITHOUT_PROGRESS`] reopens. Everything outside the loop is
//! a [`Ports`] method, so tests run it against a scripted session and
//! receiver.

use std::{future::Future, time::Duration};

use rho_protocol::{DetachReason, JobState, SessionId};
use serde_json::{Map, Value};

use super::delivery::{verdict, Answer, RetrySchedule, Verdict};
use crate::app::{
    mcp_server::webhooks::WebhookTarget,
    session_control::{FollowEnd, Followed, NewsKey, WaitResult},
};

/// How many follows in a row the daemon may drop (`lagged` or
/// `not_loaded`) before they read an event past their replay, before the
/// watch stalls.
//
// A follow that read nothing past its replay saw nothing a reopen would
// not replay, so reopening it again only spins. No path the daemon takes
// does that twice running: an observe that finds the session unloaded
// replays it from its file, settled, which is news that ends the watch, and
// a follow lags when its reader falls a whole buffer of live events behind,
// which a watcher reading on does not do again at once. Three is room for
// one of each and a spare. A drop after live events, as a long turn's
// follow lagging now and then, resets the count: that follow moved on.
const MAX_REOPENS_WITHOUT_PROGRESS: u32 = 3;

/// One watch, as its watcher runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Watching {
    pub(super) session: SessionId,
    pub(super) webhook: String,
    /// The news last reported, which is not delivered again.
    pub(super) reported: Option<NewsKey>,
}

/// How a watcher ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum End {
    /// The session settled and that was delivered: the watch is done.
    Delivered,
    /// The watch can no longer be served, and is forgotten: why.
    Dropped(String),
    /// It stopped with news undelivered or unseen: why. The watch stays
    /// until a restart or a renewal follows again.
    Stalled(String),
}

/// How a watcher connects to the daemon for a follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reconnect {
    /// Its first follow, as the server reaches the daemon.
    First,
    /// After the daemon dropped a follow: only the daemon running.
    Running,
    /// After the daemon said it is restarting in place: once it answers.
    Restarted,
}

/// Why a follow could not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Unfollowed {
    /// The daemon knows no such session.
    UnknownSession,
    Failed(String),
}

/// Why news may not be POSTed now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Denied {
    /// For good, as a revoked token or a removed target: drop the watch.
    Drop(String),
    /// The check itself failed, as with the daemon down: stall.
    Stall(String),
}

/// What the watcher records as it goes, durably, before it reads on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Record<'a> {
    /// `reported` was delivered, the receiver answering `status`.
    Delivered { reported: &'a NewsKey, status: u16 },
    /// Delivery was given up on: why.
    Failed { error: &'a str },
}

/// The world as a watcher sees it. The server's own follows the session
/// through the daemon, reads the credential store, POSTs over HTTP, and
/// writes the watch store; tests script each.
pub(super) trait Ports: Send + Sync {
    type Follow: FollowPort;

    /// Follows the watched session from its start.
    fn follow(
        &self,
        connect: Reconnect,
    ) -> impl Future<Output = Result<Self::Follow, Unfollowed>> + Send;

    /// The target to POST to, checked right before every POST: the token
    /// still exists, the target is still bound to it, and the session is
    /// still in the token's reach.
    fn authorize(&self) -> impl Future<Output = Result<WebhookTarget, Denied>> + Send;

    fn post(&self, target: &WebhookTarget, body: &Value) -> impl Future<Output = Answer> + Send;

    /// Waits `gap` before a retry.
    fn sleep(&self, gap: Duration) -> impl Future<Output = ()> + Send;

    /// Records `record`, returning once it is durable.
    fn record(&self, record: Record<'_>) -> impl Future<Output = ()> + Send;
}

/// A follow of the watched session (`session_control::Follow`).
pub(super) trait FollowPort: Send {
    fn next(&mut self) -> impl Future<Output = Followed> + Send;

    /// Whether the follow read an event past its replay.
    fn progressed(&self) -> bool;
}

/// What a piece of news is to a watch: the one test of what to deliver
/// and what ends the watch, for the watcher's news and the `watch` call's
/// answer alike.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum News {
    /// Nothing to tell: the session is not done, or waits with nothing
    /// open, as it does between a request's answer and its turn running
    /// again (or before the request's own event arrived).
    Nothing,
    /// It waits for a person to answer what it has open: delivered.
    Asks(NewsKey),
    /// It settled, with nothing left for a person to answer: delivered,
    /// and the watch ends.
    Settled(NewsKey),
}

impl News {
    pub(super) fn of(news: &WaitResult) -> Self {
        if !news.done {
            return Self::Nothing;
        }
        match (news.inputs.is_empty(), news.state) {
            (false, _) => Self::Asks(NewsKey::of(news)),
            (true, JobState::WaitingForInput) => Self::Nothing,
            (true, _) => Self::Settled(NewsKey::of(news)),
        }
    }
}

/// The webhook body: the news as `wait` returns it, and the server it is
/// from. The `watch` call answers the same, with `watching` added.
pub(super) fn body(server: &str, news: &WaitResult) -> Map<String, Value> {
    let mut body = match serde_json::to_value(news) {
        Ok(Value::Object(body)) => body,
        _ => unreachable!("a wait result encodes as a JSON object"),
    };
    body.insert("server".to_owned(), Value::String(server.to_owned()));
    body
}

/// Runs `watching` until it ends.
pub(super) async fn run<P: Ports>(
    ports: &P,
    schedule: RetrySchedule,
    server: &str,
    mut watching: Watching,
) -> End {
    let mut connect = Reconnect::First;
    // Follows in a row the daemon dropped before they read anything past
    // their replay.
    let mut stuck: u32 = 0;
    loop {
        let mut follow = match ports.follow(connect).await {
            Ok(follow) => follow,
            Err(Unfollowed::UnknownSession) => {
                return End::Dropped(format!(
                    "no session {} is known to the rho daemon",
                    watching.session
                ))
            }
            Err(Unfollowed::Failed(error)) => {
                return End::Stalled(format!(
                    "could not follow session {}: {error}",
                    watching.session
                ))
            }
        };
        connect = loop {
            let news = match follow.next().await {
                Followed::News(news) => news,
                Followed::Ended(FollowEnd::Detached(
                    reason @ (DetachReason::Lagged | DetachReason::NotLoaded),
                )) => {
                    if follow.progressed() {
                        stuck = 0;
                    } else if stuck == MAX_REOPENS_WITHOUT_PROGRESS {
                        return End::Stalled(format!(
                            "the rho daemon ended {} follows in a row before they read anything new, the last {reason:?}; a watch reopens at most MAX_REOPENS_WITHOUT_PROGRESS ({MAX_REOPENS_WITHOUT_PROGRESS}) times without progress",
                            stuck + 1
                        ));
                    } else {
                        stuck += 1;
                    }
                    break Reconnect::Running;
                }
                Followed::Ended(FollowEnd::Detached(DetachReason::Restarting)) => {
                    break Reconnect::Restarted
                }
                Followed::Ended(FollowEnd::Detached(DetachReason::Deleted)) => {
                    return End::Dropped(format!("session {} was deleted", watching.session))
                }
                Followed::Ended(FollowEnd::Detached(DetachReason::DaemonStopping)) => {
                    return End::Stalled("the rho daemon stopped".to_owned())
                }
                Followed::Ended(FollowEnd::Detached(reason)) => {
                    return End::Stalled(format!("the rho daemon ended the follow ({reason:?})"))
                }
                Followed::Ended(FollowEnd::Closed) => {
                    return End::Stalled("the connection to the rho daemon closed".to_owned())
                }
            };
            let (about, settled) = match News::of(&news) {
                News::Nothing => continue,
                News::Asks(about) => (about, false),
                News::Settled(about) => (about, true),
            };
            if watching.reported.as_ref() != Some(&about) {
                match deliver(ports, schedule, server, &watching.webhook, &news).await {
                    Ok(status) => {
                        ports
                            .record(Record::Delivered {
                                reported: &about,
                                status,
                            })
                            .await;
                    }
                    Err(end) => return end,
                }
                watching.reported = Some(about);
            }
            if settled {
                return End::Delivered;
            }
        };
    }
}

/// POSTs `news` to the watch's webhook until the receiver takes it, the
/// answer says it never will, or the schedule runs out; the status it took
/// it with, or how the watch ends.
async fn deliver<P: Ports>(
    ports: &P,
    schedule: RetrySchedule,
    server: &str,
    webhook: &str,
    news: &WaitResult,
) -> Result<u16, End> {
    let body = Value::Object(body(server, news));
    let mut gaps = schedule.gaps();
    let mut attempts: u32 = 0;
    // The gaps slept between attempts, which the attempts' own time adds to.
    let mut waited = Duration::ZERO;
    let error = loop {
        let target = match ports.authorize().await {
            Ok(target) => target,
            Err(Denied::Drop(why)) => return Err(End::Dropped(why)),
            Err(Denied::Stall(why)) => return Err(End::Stalled(why)),
        };
        attempts += 1;
        let answer = ports.post(&target, &body).await;
        match verdict(&answer) {
            Verdict::Delivered(status) => return Ok(status),
            Verdict::Retry => match gaps.next() {
                Some(gap) => {
                    ports.sleep(gap).await;
                    waited += gap;
                }
                None => {
                    break format!(
                        "webhook {webhook:?} not delivered after {attempts} attempts, {} min of retry gaps between them: last {answer}",
                        waited.as_secs() / 60
                    )
                }
            },
            Verdict::Stall => break format!("webhook {webhook:?} refused delivery: {answer}"),
        }
    };
    tracing::error!(session = %news.session, %error, "could not deliver a watched session's news");
    ports.record(Record::Failed { error: &error }).await;
    Err(End::Stalled(error))
}

#[cfg(test)]
#[path = "watcher_tests.rs"]
mod tests;
