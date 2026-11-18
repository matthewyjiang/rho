//! A watch's [`Ports`] on the server: its session through the daemon, or
//! through its remote's `wait`, the tokens, targets, and remotes in the
//! credential store, HTTP, and the watch store.

use std::{sync::Arc, time::Duration};

use rho_protocol::ClientEnvironment;
use serde_json::Value;

use super::{
    book::Key,
    delivery::Answer,
    lock,
    remote_follow::{RemoteFollow, ServerWaits, REACH_RETRY},
    watcher::{Denied, End, FollowPort, Ports, Reconnect, Record, Unfollowed},
    Watches,
};
use crate::{
    app::{
        mcp_server::{
            remotes::RemoteSet,
            tokens::TokenSet,
            webhooks::{WebhookSet, WebhookTarget},
        },
        session_control::{Caller, Connect, ControlError, DaemonUse, Follow, Followed, Reach},
    },
    caller_context::CallerContext,
    credential_store::UntrackedCredentialStore,
};

/// Unix time, in seconds.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// One watch's ports: the watch's key, webhook, and remote, and the task
/// generation running it.
pub(super) struct ServerPorts {
    pub(super) watches: Arc<Watches>,
    pub(super) key: Key,
    pub(super) webhook: String,
    /// The remote the session is on, by name; none for this server's own.
    pub(super) server: Option<String>,
    pub(super) generation: u64,
}

impl ServerPorts {
    /// Applies how the watch ended, unless it was replaced or removed
    /// meanwhile, and saves that.
    pub(super) async fn finish(&self, end: End) {
        let snapshot = {
            let mut state = lock(&self.watches.state);
            if !state.book.apply_end(&self.key, self.generation, &end) {
                return;
            }
            state.tasks.remove(&self.key);
            state.book.snapshot()
        };
        let (token, session) = &self.key;
        match &end {
            End::Delivered => {
                tracing::info!(%session, %token, "a watched session settled and its news was delivered");
            }
            End::Dropped(reason) => {
                tracing::warn!(%session, %token, %reason, "dropped a webhook watch");
            }
            End::Stalled(error) => {
                tracing::warn!(%session, %token, %error, "a webhook watch stalled");
            }
        }
        self.watches.save(snapshot).await;
    }
}

/// A watch's follow of its session, on this server's daemon or a remote.
pub(super) enum ServerFollow {
    Local(Box<Follow>),
    Remote(Box<RemoteFollow<ServerWaits>>),
}

impl FollowPort for ServerFollow {
    async fn next(&mut self) -> Result<Followed, Denied> {
        match self {
            Self::Local(follow) => Ok(follow.next().await),
            Self::Remote(follow) => follow.next().await.map(Followed::News),
        }
    }

    fn progressed(&self) -> bool {
        match self {
            Self::Local(follow) => follow.progressed(),
            // Never asked: a remote's follow ends only with an error, which
            // ends the watch, never with a detach to reopen after.
            Self::Remote(_) => true,
        }
    }
}

impl Ports for ServerPorts {
    type Follow = ServerFollow;

    async fn follow(&self, connect: Reconnect) -> Result<ServerFollow, Unfollowed> {
        if let Some(server) = &self.server {
            let waits = ServerWaits {
                server: server.clone(),
                session: self.key.1.clone(),
            };
            return Ok(ServerFollow::Remote(Box::new(RemoteFollow::new(
                waits,
                REACH_RETRY,
            ))));
        }
        let control = &self.watches.control;
        let connect = match connect {
            // As the server reaches the daemon: a watch's first follow
            // comes of a call, or of the server starting, which are its
            // owner's acts. Starting the daemon then is how news that came
            // while the machine was down, such as a turn `interrupted` by a
            // reboot, still reaches the webhook. Every reconnect after, and
            // every check before a POST, reaches only a daemon running.
            Reconnect::First => Connect::Reach(control.reach()),
            Reconnect::Running => Connect::Reach(Reach::Running),
            Reconnect::Restarted => Connect::Restarted,
        };
        match control.follow(&self.key.1, connect).await {
            Ok(follow) => Ok(ServerFollow::Local(Box::new(follow))),
            Err(error) if error.is_unknown_session() => Err(Unfollowed::UnknownSession),
            Err(error) => Err(Unfollowed::Failed(error.to_string())),
        }
    }

    async fn authorize(&self) -> Result<WebhookTarget, Denied> {
        let sets = tokio::task::spawn_blocking(|| {
            anyhow::Ok((
                TokenSet::load(&UntrackedCredentialStore)?,
                WebhookSet::load(&UntrackedCredentialStore)?,
                RemoteSet::load(&UntrackedCredentialStore)?,
            ))
        })
        .await;
        let (tokens, webhooks, remotes) = match sets {
            Ok(Ok(sets)) => sets,
            Ok(Err(error)) => return Err(Denied::Stall(format!("{error:#}"))),
            Err(error) => return Err(Denied::Stall(error.to_string())),
        };
        let (name, session) = &self.key;
        let Some(token) = tokens.tokens.iter().find(|token| &token.name == name) else {
            return Err(Denied::Drop(format!("token {name:?} was revoked")));
        };
        let target = webhooks
            .for_token(name, &self.webhook)
            .map_err(|unbound| Denied::Drop(unbound.to_string()))?
            .clone();
        if let Some(server) = &self.server {
            // The remote holds the hub to its workspaces on every `wait`
            // the follow makes; here it only has to be there still.
            remotes.require(server).map_err(Denied::Drop)?;
            return Ok(target);
        }
        let caller = Caller::new(
            token.grant(),
            &CallerContext::ROOT,
            ClientEnvironment::default(),
        );
        // Never starts the daemon: a stopped one stalls the watch until
        // the server restarts or the session is watched again.
        match self
            .watches
            .control
            .check_follow(&caller, session, DaemonUse::RunningOnly)
            .await
        {
            Ok(()) => Ok(target),
            Err(error @ (ControlError::Refused(_) | ControlError::UnknownSession(_))) => {
                Err(Denied::Drop(error.to_string()))
            }
            Err(error @ (ControlError::Invalid(_) | ControlError::Daemon(_))) => Err(
                Denied::Stall(format!("could not check the session's reach: {error}")),
            ),
        }
    }

    async fn post(&self, target: &WebhookTarget, body: &Value) -> Answer {
        self.watches.poster.post(target, body).await
    }

    async fn sleep(&self, gap: Duration) {
        tokio::time::sleep(gap).await;
    }

    async fn record(&self, record: Record<'_>) {
        let snapshot = {
            let mut state = lock(&self.watches.state);
            if !state
                .book
                .apply_record(&self.key, self.generation, record, unix_now())
            {
                return;
            }
            state.book.snapshot()
        };
        self.watches.save(snapshot).await;
    }
}
