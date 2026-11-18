//! Webhook watches for `rho mcp serve --http`: a caller asks to hear of a
//! session at a webhook target the owner configured (`webhooks.rs`), with
//! `watch` or with `notify` on `new` and `prompt`, and the server POSTs the
//! session's news there: each time it stops to wait for a person, and once
//! when it settles, which ends the watch. `notify` on the next `prompt`
//! renews it.
//!
//! A watch is one-shot because it follows the session, and a followed
//! session stays loaded in the daemon: watching only while the session is
//! unsettled lets the daemon unload it once it is done. An unloaded session
//! cannot be followed, and needs nothing watched until a prompt starts it
//! again.
//!
//! This is a layer of the HTTP adapter, not a core action: the core and
//! the native `sessions` tool know nothing of it, and nothing leaves the
//! daemon. Each call is checked as the core checks `wait` (and `new` or
//! `prompt` for `notify`), and the token may use only targets bound to it,
//! up to `--max-watches` watches ([`default_max_watches`]). One server per
//! rho home keeps the watches (`store.rs`); a server that does not, because
//! another one does, is [`WatchMode::Off`] and refuses them. Each watch
//! runs as its own task (`watcher.rs`) on its own connection to the daemon,
//! and before every POST the server reads the tokens and targets again and
//! checks the session is still in the token's reach, so a revoke or a
//! removed target drops the watch.
//!
//! What the watches are, and each budget slot a call holds while it works,
//! is the [`WatchBook`]'s, a pure type under one mutex; this module runs
//! what it decides: the calls, the watchers' tasks, and the writes of its
//! snapshots, which wait on the disk off that mutex.
//!
//! A hub watches its remotes' sessions too (`remote_follow.rs`): it follows
//! each through its remote's `wait` and POSTs its news itself, naming the
//! remote as `server`, so a remote needs no webhook of its own.

use std::{
    collections::HashMap,
    fs::File,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use rho_protocol::SessionId;
use serde_json::{Map, Value};
use tokio::task::AbortHandle;

use super::{
    remotes::{control_lasts, Lasts, RouteError, Target},
    token_store,
    tool::ServedTool,
    webhooks::{Unbound, WebhookSet},
};
use crate::app::session_control::{
    parse_arguments, Action, Caller, ControlError, Holder, SessionControl,
};

mod book;
mod delivery;
mod ports;
mod remote_follow;
mod store;
mod tools;
mod watcher;

use book::{Key, OverBudget, Slot, Start, WatchBook, Watched};
use delivery::{HttpPoster, RetrySchedule};
use ports::ServerPorts;
pub(crate) use store::{DeliveryRecord, DeliveryResult, WatchFile, WatchStore, DIR};
use store::{Snapshot, Writer};
use tools::{take_notify, UnwatchParams, WatchParams};
pub(crate) use tools::{tools as watch_tools, with_notify};

/// How many sessions each token may watch at once when `--max-watches`
/// is not given: [`WATCHES_PER_SESSION`] for each session it may run
/// (`--max-sessions`).
pub(crate) fn default_max_watches(max_sessions: u32) -> u32 {
    max_sessions.saturating_mul(WATCHES_PER_SESSION)
}

// Provisional, a tripwire against a runaway loop, not a capacity: a watch
// costs one observe connection to the daemon and one task while its
// session is unsettled, and ends when it settles; a stalled one is only
// its row in `watches.json`. Five per session a token may run lets a bot
// watch every session it runs and the ones people run beside them, as the
// owner raises `--max-sessions` too, while a loop stops at a count `rho
// mcp webhook list` shows beside each token's watches.
const WATCHES_PER_SESSION: u32 = 5;

/// What a call's result says, as `watch_error`, when the watches could not
/// be saved. The path and the cause go to the server's log only: the
/// result may cross the internet, and the path names the local user.
const NOT_SAVED: &str = "the server could not save its webhook watches, so a restart of the \
    server will not keep this change; the server's log says why";

/// Why a watch call was refused or failed. Its `Display` is what the
/// caller sees.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WatchError {
    #[error(transparent)]
    Control(#[from] ControlError),
    #[error(transparent)]
    Route(#[from] RouteError),
    #[error(transparent)]
    Unbound(#[from] Unbound),
    #[error(transparent)]
    Budget(#[from] OverBudget),
    /// Another server keeps this rho home's watches ([`WatchMode::Off`]);
    /// the lock's path is in this server's log.
    #[error(
        "this server keeps no webhook watches: another `rho mcp serve --http` on this rho home keeps them"
    )]
    Off,
    #[error("{0}")]
    Failed(String),
}

impl WatchError {
    /// Whether calling again could change the error: not when the call was
    /// refused or named what is not there.
    pub(crate) fn lasts(&self) -> Lasts {
        match self {
            Self::Control(error) => control_lasts(error),
            Self::Route(error) => error.lasts(),
            Self::Unbound(_) | Self::Off => Lasts::ForGood,
            Self::Budget(_) | Self::Failed(_) => Lasts::ForNow,
        }
    }
}

/// Whether the HTTP server keeps the rho home's watches.
#[derive(Clone)]
pub(crate) enum WatchMode {
    /// It does, and offers `watch`, `unwatch`, and `notify`.
    On(Arc<Watches>),
    /// Another server does: this one lists none of them and refuses them.
    Off,
}

impl WatchMode {
    /// Answers `tool` for `caller` on `target`: the watch tools and a call
    /// with `notify` with the watches, or refused when off; every other
    /// call where `target` is.
    pub(crate) async fn call(
        &self,
        control: &SessionControl,
        caller: &Caller,
        tool: ServedTool,
        mut arguments: Map<String, Value>,
        target: &Target,
    ) -> Result<Value, WatchError> {
        match self {
            Self::On(watches) => watches.call(caller, tool, arguments, target).await,
            Self::Off => match tool {
                ServedTool::Watch | ServedTool::Unwatch => Err(WatchError::Off),
                ServedTool::Core(action) => match take_notify(action, &mut arguments)? {
                    Some(_) => Err(WatchError::Off),
                    None => Ok(target.call(control, caller, action, arguments).await?),
                },
            },
        }
    }
}

/// The HTTP server's watches, which it keeps.
pub(crate) struct Watches {
    control: Arc<SessionControl>,
    /// The name news says it is from (`--name`).
    server: String,
    schedule: RetrySchedule,
    poster: HttpPoster,
    writer: Arc<Writer>,
    state: Mutex<State>,
}

struct State {
    book: WatchBook,
    /// The task running each watch the book runs.
    tasks: HashMap<Key, AbortHandle>,
    /// The store's lock, held for the server's life.
    _lock: File,
}

impl State {
    /// Stops the task running watch `key`, if one does.
    fn abort(&mut self, key: &Key) {
        if let Some(task) = self.tasks.remove(key) {
            task.abort();
        }
    }
}

impl Watches {
    /// Takes the watches in `dir` (`<rho home>/mcp-serve`) and resumes each
    /// one, for a server called `server`. When another server holds them,
    /// this one serves on and refuses watches ([`WatchMode::Off`]). A file
    /// that does not parse is an error.
    pub(crate) fn open(
        control: Arc<SessionControl>,
        server: String,
        max_watches: u32,
        dir: PathBuf,
    ) -> anyhow::Result<WatchMode> {
        let store = WatchStore::new(dir);
        let Some(held) = store.lock()? else {
            tracing::warn!(
                lock = %store.lock_path().display(),
                "another rho mcp serve --http keeps this rho home's webhook watches; this one refuses watch and notify"
            );
            return Ok(WatchMode::Off);
        };
        let writer = Writer::new(store.clone());
        let (mut book, starts) = WatchBook::resume(store.load()?, max_watches);
        // Before the server serves or any watch runs, so nothing waits on
        // this write.
        writer.write_now(&book.snapshot())?;
        let watches = Arc::new(Self {
            control,
            server,
            schedule: RetrySchedule::DEFAULT,
            poster: HttpPoster::new()?,
            writer,
            state: Mutex::new(State {
                book,
                tasks: HashMap::new(),
                _lock: held,
            }),
        });
        {
            let mut state = lock(&watches.state);
            for start in starts {
                watches.spawn(&mut state, start);
            }
        }
        Ok(WatchMode::On(watches))
    }

    /// The name news says it is from: the remote's the session is on, or
    /// this server's.
    fn name_of<'a>(&'a self, remote: Option<&'a str>) -> &'a str {
        remote.unwrap_or(&self.server)
    }

    /// What the server's instructions add about watches.
    pub(crate) fn instructions(&self) -> String {
        format!(
            "This server is {:?}. `watch`, or `notify` on `new` and `prompt`, has it POST a \
             session's news to a webhook, naming the server as `server`: when the session \
             waits for a person, and once when it settles, which ends the watch.",
            self.server
        )
    }

    /// Answers `tool` for `caller` on `target`: the watch tools and a call
    /// with `notify` here, every other call where `target` is.
    pub(crate) async fn call(
        self: &Arc<Self>,
        caller: &Caller,
        tool: ServedTool,
        mut arguments: Map<String, Value>,
        target: &Target,
    ) -> Result<Value, WatchError> {
        match tool {
            ServedTool::Watch => {
                self.watch(
                    caller,
                    parse_arguments(ServedTool::Watch.name(), arguments)?,
                    target,
                )
                .await
            }
            ServedTool::Unwatch => {
                self.unwatch(
                    caller,
                    parse_arguments(ServedTool::Unwatch.name(), arguments)?,
                )
                .await
            }
            ServedTool::Core(action) => match take_notify(action, &mut arguments)? {
                Some(webhook) => {
                    self.notify(caller, action, arguments, webhook, target)
                        .await
                }
                None => Ok(target
                    .call(&self.control, caller, action, arguments)
                    .await?),
            },
        }
    }

    /// `watch`: where the session is now, and a watch unless it settled.
    /// A watch the session does not need, as of one that settled, takes no
    /// slot of the budget, and a call that changes nothing saves nothing.
    async fn watch(
        self: &Arc<Self>,
        caller: &Caller,
        params: WatchParams,
        target: &Target,
    ) -> Result<Value, WatchError> {
        let token = token_of(caller)?;
        target
            .check_follow(&self.control, caller, &params.session)
            .await?;
        load_webhooks().await?.for_token(token, &params.webhook)?;
        let key = (token.to_owned(), params.session);
        // Before `now`, so a call that changes the watch meanwhile, as a
        // `notify` starting a turn after `now` saw the session settled,
        // decides it.
        let read = lock(&self.state).book.revision(&key);
        let now = target.now(&self.control, caller, &key.1).await?;
        let server = target.remote_name().map(str::to_owned);
        let (watching, snapshot) = {
            let mut state = lock(&self.state);
            match state
                .book
                .watch(token, params.webhook, server, &now, read)?
            {
                Watched::Unchanged { watching } => (watching, None),
                Watched::Removed => {
                    state.abort(&key);
                    (false, Some(state.book.snapshot()))
                }
                Watched::Started(start) => {
                    self.spawn(&mut state, start);
                    (true, Some(state.book.snapshot()))
                }
            }
        };
        let mut result = watcher::body(self.name_of(target.remote_name()), &now);
        result.insert("watching".to_owned(), Value::Bool(watching));
        if let Some(snapshot) = snapshot {
            self.save_into(snapshot, &mut result).await;
        }
        Ok(Value::Object(result))
    }

    /// `unwatch`: forgets the caller's watch of the session, if any.
    async fn unwatch(&self, caller: &Caller, params: UnwatchParams) -> Result<Value, WatchError> {
        let token = token_of(caller)?;
        let mut result = Map::new();
        result.insert("watching".to_owned(), Value::Bool(false));
        let snapshot = {
            let mut state = lock(&self.state);
            let key = (token.to_owned(), params.session);
            state.abort(&key);
            state.book.remove(&key).then(|| state.book.snapshot())
        };
        if let Some(snapshot) = snapshot {
            self.save_into(snapshot, &mut result).await;
        }
        Ok(Value::Object(result))
    }

    /// `new` or `prompt` with `notify`: checks the target and the budget
    /// first, so a refused notify starts nothing, then runs the call and
    /// watches the session it started a turn on. Once the turn started, it
    /// answers with the call's result whatever else fails, so a caller
    /// never starts the turn again: `watch_error` says what did.
    async fn notify(
        self: &Arc<Self>,
        caller: &Caller,
        action: Action,
        arguments: Map<String, Value>,
        webhook: String,
        target: &Target,
    ) -> Result<Value, WatchError> {
        let token = token_of(caller)?;
        load_webhooks().await?.for_token(token, &webhook)?;
        // A prompt renews the session's watch; a new session adds one.
        let prompted = arguments
            .get("session")
            .and_then(Value::as_str)
            .filter(|_| action == Action::Prompt)
            .map(SessionId::new);
        let reservation = Reservation::hold(&self.state, token, prompted.as_ref())?;
        let mut result = match target
            .call(&self.control, caller, action, arguments)
            .await?
        {
            Value::Object(result) => result,
            other => return Ok(other),
        };
        let session = result
            .get("session")
            .and_then(Value::as_str)
            .map(SessionId::new);
        let Some(session) = session else {
            tracing::error!(
                action = action.name(),
                "the core's result named no session, so nothing is watched"
            );
            result.insert("watching".to_owned(), Value::Bool(false));
            result.insert(
                "watch_error".to_owned(),
                Value::String(format!("`{}` named no session to watch", action.name())),
            );
            return Ok(Value::Object(result));
        };
        let snapshot = {
            let mut state = lock(&self.state);
            let server = target.remote_name().map(str::to_owned);
            let start = state
                .book
                .notify(reservation.take(), session, webhook, server);
            self.spawn(&mut state, start);
            state.book.snapshot()
        };
        result.insert("watching".to_owned(), Value::Bool(true));
        self.save_into(snapshot, &mut result).await;
        Ok(Value::Object(result))
    }

    /// Runs `start` as a task of its own, in place of any task that ran
    /// its watch.
    fn spawn(self: &Arc<Self>, state: &mut State, start: Start) {
        let Start {
            key,
            generation,
            watching,
        } = start;
        state.abort(&key);
        let ports = ServerPorts {
            watches: Arc::clone(self),
            key: key.clone(),
            webhook: watching.webhook.clone(),
            server: watching.server.clone(),
            generation,
        };
        let task = tokio::spawn(async move {
            let watches = &ports.watches;
            let server = watches.name_of(ports.server.as_deref());
            let end = watcher::run(&ports, watches.schedule, server, watching).await;
            ports.finish(end).await;
        });
        state.tasks.insert(key, task.abort_handle());
    }

    /// Writes `snapshot`, returning once it is on disk; whether it is. A
    /// failure is logged with its path and cause; the caller hears only
    /// that it failed.
    async fn save(&self, snapshot: Snapshot) -> bool {
        match self.writer.write(snapshot).await {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(error = %format!("{error:#}"), "could not save the webhook watches");
                false
            }
        }
    }

    /// [`Self::save`], noting a failure in a call's `result`.
    async fn save_into(&self, snapshot: Snapshot, result: &mut Map<String, Value>) {
        if !self.save(snapshot).await {
            result.insert(
                "watch_error".to_owned(),
                Value::String(NOT_SAVED.to_owned()),
            );
        }
    }
}

/// A slot of a token's budget a `notify` call holds while the core starts
/// its turn, given back unless the call starts its watch with it.
struct Reservation<'a> {
    state: &'a Mutex<State>,
    slot: Option<Slot>,
}

impl<'a> Reservation<'a> {
    /// Holds a slot of `token`'s budget for a watch of `session`, or of
    /// the session `new` makes; refused past the budget.
    fn hold(
        state: &'a Mutex<State>,
        token: &str,
        session: Option<&SessionId>,
    ) -> Result<Self, OverBudget> {
        let slot = lock(state).book.reserve(token, session)?;
        Ok(Self {
            state,
            slot: Some(slot),
        })
    }

    /// The slot, for the book to start a watch with while the state is
    /// locked.
    fn take(mut self) -> Slot {
        self.slot
            .take()
            .expect("a reservation holds its slot until taken")
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            lock(self.state).book.release(slot);
        }
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The token a call came with: every HTTP call has one.
fn token_of(caller: &Caller) -> Result<&str, WatchError> {
    match &caller.grant.holder {
        Holder::Token(name) => Ok(name),
        Holder::Local | Holder::Session(_) => Err(WatchError::Failed(
            "webhook watches are for HTTP tokens".to_owned(),
        )),
    }
}

/// The saved targets, read past the store's cache as tokens are. A
/// failure's cause goes to the server's log only, as a save's does.
async fn load_webhooks() -> Result<WebhookSet, WatchError> {
    token_store::load_fresh::<WebhookSet>()
        .await
        .map_err(|error| {
            tracing::error!(error = %format!("{error:#}"), "could not read the webhook targets");
            WatchError::Failed(
                "the server could not read its webhook targets; the server's log says why"
                    .to_owned(),
            )
        })
}
