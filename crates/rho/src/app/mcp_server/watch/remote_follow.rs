//! A watch's follow of a session on a remote: the remote's `wait`, called
//! again and again from the cursor it returns, which reports each new
//! reason the session is done, as a follow of this server's daemon does.
//!
//! Each `wait` reads the remote as it is configured then, so a `rho mcp
//! remote add` that rotated its secret or moved it applies to the watches
//! already running. A remote it cannot reach, or that refuses the secret,
//! as for a moment while `rho mcp remote add` rotates it, is tried again on
//! [`REACH_RETRY`], so one that reboots is followed again once it is back.
//! A remote that answers the call with an error, as for a session it does
//! not know, or that was removed, ends the watch, as the same answers from
//! this server's daemon do.
//!
//! Some answers come back with nothing new: a session waiting for a person
//! repeats the news it was done with at once, and a `wait` that ends with
//! `done: false` from the same cursor, as when the remote's daemon dropped
//! its follow or the turn was quiet throughout, has nothing to resume from.
//! Both are asked again only after [`IDLE_POLL`]. Everything outside the loop is a
//! [`RemoteWaits`] method, so tests run it against a scripted remote.

use std::{future::Future, time::Duration};

use rho_protocol::SessionId;

use super::{delivery::RetrySchedule, watcher::Denied};
use crate::{
    app::{
        mcp_server::{
            remotes::{self, Asked, Lasts, RemoteError, RemoteSet},
            token_store,
        },
        session_control::{NewsKey, WaitResult, MAX_WAIT_SECS},
    },
    caller_context::CallerContext,
};

/// How long a follow waits before it asks again when a `wait` answered at
/// once with nothing new.
//
// A person answering takes seconds at least, and the next `wait` follows
// the turn they start from the cursor, so its news is not missed, only
// reported up to this late when the session settles within it. At ten
// seconds a waiting watch costs the remote one connection that often.
const IDLE_POLL: Duration = Duration::from_secs(10);

/// When a follow asks an unreachable remote again: gaps doubling from two
/// seconds to a minute, for an hour of gaps. Past it the watch stalls,
/// with the error `rho mcp webhook list` shows, until the server restarts
/// or the session is watched again.
//
// A reboot or a network change takes minutes; an hour also outlasts a
// slow update. Capping the gap at a minute finds a remote back within one,
// at about sixty connection attempts an hour; each one that hangs adds at
// most the call's own timeout, 70 s, to the hour.
pub(super) const REACH_RETRY: RetrySchedule = RetrySchedule {
    first_gap: Duration::from_secs(2),
    max_gap: Duration::from_secs(60),
    horizon: Duration::from_secs(60 * 60),
};

/// Why one `wait` on the remote failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WaitFailed {
    /// Asking again may work: why.
    Again(String),
    /// It never will, for this session: why.
    Ended(String),
}

impl From<RemoteError> for WaitFailed {
    /// A refused secret may answer later: `rho mcp remote add` rotates the
    /// remote's token before it saves the new secret here, and a `wait`
    /// between the two is refused. So may an error the remote answers with,
    /// as with its daemon down, unless it says the error lasts, as when
    /// the session is one it does not know: that ends the follow, as the
    /// same answer from this server's daemon does.
    fn from(error: RemoteError) -> Self {
        match error {
            RemoteError::Unreachable { .. }
            | RemoteError::Refused { .. }
            | RemoteError::Failed {
                lasts: Lasts::ForNow,
                ..
            } => Self::Again(error.to_string()),
            RemoteError::Failed {
                lasts: Lasts::ForGood,
                ..
            } => Self::Ended(error.to_string()),
        }
    }
}

/// The remote as a follow sees it. The server's reads the remote's
/// configuration and calls its `wait`; tests script both.
pub(super) trait RemoteWaits: Send + Sync {
    /// One `wait` on the session, as long as one may, from `cursor`.
    fn wait(
        &self,
        cursor: Option<&str>,
    ) -> impl Future<Output = Result<WaitResult, WaitFailed>> + Send;

    fn sleep(&self, gap: Duration) -> impl Future<Output = ()> + Send;
}

/// A follow of one session on a remote.
pub(super) struct RemoteFollow<W> {
    waits: W,
    schedule: RetrySchedule,
    /// The cursor the last `wait` returned.
    cursor: Option<String>,
    /// The news last reported, while the session stays done with it.
    reported: Option<NewsKey>,
}

impl<W: RemoteWaits> RemoteFollow<W> {
    pub(super) fn new(waits: W, schedule: RetrySchedule) -> Self {
        Self {
            waits,
            schedule,
            cursor: None,
            reported: None,
        }
    }

    /// The session's next news, the first being where it is when it is
    /// done already; or why the follow cannot go on.
    pub(super) async fn next(&mut self) -> Result<WaitResult, Denied> {
        let mut gaps = self.schedule.gaps();
        let mut attempts: u32 = 0;
        loop {
            attempts += 1;
            let news = match self.waits.wait(self.cursor.as_deref()).await {
                Ok(news) => news,
                Err(WaitFailed::Again(why)) => match gaps.next() {
                    Some(gap) => {
                        self.waits.sleep(gap).await;
                        continue;
                    }
                    None => return Err(Denied::Stall(format!("{why}, after {attempts} attempts"))),
                },
                Err(WaitFailed::Ended(why)) => return Err(Denied::Drop(why)),
            };
            gaps = self.schedule.gaps();
            attempts = 0;
            let moved = news.cursor.is_some() && news.cursor != self.cursor;
            if moved {
                self.cursor.clone_from(&news.cursor);
            }
            if !news.done {
                // What it was done with is news again once it is not.
                self.reported = None;
                if !moved {
                    self.waits.sleep(IDLE_POLL).await;
                }
                continue;
            }
            let key = NewsKey::of(&news);
            if self.reported.as_ref() == Some(&key) {
                self.waits.sleep(IDLE_POLL).await;
                continue;
            }
            self.reported = Some(key);
            return Ok(news);
        }
    }
}

/// The server's [`RemoteWaits`]: remote `server`'s `wait` on `session`.
pub(super) struct ServerWaits {
    pub(super) server: String,
    pub(super) session: SessionId,
}

impl RemoteWaits for ServerWaits {
    async fn wait(&self, cursor: Option<&str>) -> Result<WaitResult, WaitFailed> {
        let remotes = token_store::load_fresh::<RemoteSet>()
            .await
            .map_err(|error| WaitFailed::Again(format!("{error:#}")))?;
        let remote = remotes.require(&self.server).map_err(WaitFailed::Ended)?;
        // A read on nobody's turn: the remote's token for the hub decides.
        remotes::wait(
            remote,
            &self.session,
            MAX_WAIT_SECS,
            cursor,
            &CallerContext::ROOT,
            Asked::ByWatch,
        )
        .await
        .map_err(WaitFailed::from)
    }

    async fn sleep(&self, gap: Duration) {
        tokio::time::sleep(gap).await;
    }
}

#[cfg(test)]
#[path = "remote_follow_tests.rs"]
mod tests;
