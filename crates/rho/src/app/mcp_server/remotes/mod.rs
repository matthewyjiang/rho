//! A hub: one `rho mcp serve` that drives other machines' sessions as well
//! as its own. The owner adds each machine's `rho mcp serve --http` as a
//! remote (`rho mcp remote add`), with a token that machine issued the hub,
//! and every tool then takes `server`, the machine to act on: this server's
//! own name (`--name`, the host name by default) or a remote's. `list`
//! without one lists every server.
//!
//! The hub routes; it decides nothing a remote decides. A call for a remote
//! is checked against the caller's access here, then sent to the remote,
//! whose token holds it to that machine's workspaces and limits. The
//! caller's depth and access travel with it (`forwarded`); its workspaces
//! do not, since they are paths on this machine. Every token reaches every
//! remote: on a remote, the remote's token for the hub decides what it may
//! touch. A forwarded call is served where it lands and routed no further
//! (`client.rs`).
//!
//! Remotes are read from the credential store on every call, as tokens are
//! ([`Machines`]), so adding or removing one applies from the next call,
//! and a host sees the new `server` choices when it lists the tools again.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use futures_util::future::join_all;
use rho_protocol::SessionId;
use serde_json::{Map, Value};

use super::{token_store, tool::ServedTool};
use crate::{
    app::session_control::{Action, Caller, ControlError, DaemonUse, SessionControl, WaitResult},
    caller_context::{CallerContext, Scope, Workspaces},
};

mod client;
mod store;

pub(crate) use client::{call, wait, Asked, Lasts, RemoteError, CALL_TIMEOUT, FORWARDED, LASTING};
pub(crate) use store::{check_name, check_url, Remote, RemoteSet};

/// The argument that names a call's server.
const SERVER: &str = "server";

/// Why a call could not be routed or failed where it went. Its `Display`
/// is what the caller sees.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RouteError {
    /// The call named no server, or one there is not.
    #[error("{0}")]
    Server(String),
    #[error(transparent)]
    Control(#[from] ControlError),
    #[error(transparent)]
    Remote(#[from] RemoteError),
}

impl RouteError {
    /// Whether calling again could change the error: not when the call was
    /// refused, named no server there is, or named a session the daemon
    /// does not know.
    pub(crate) fn lasts(&self) -> Lasts {
        match self {
            Self::Server(_) => Lasts::ForGood,
            Self::Control(error) => control_lasts(error),
            Self::Remote(RemoteError::Failed { lasts, .. }) => *lasts,
            Self::Remote(RemoteError::Unreachable { .. } | RemoteError::Refused { .. }) => {
                Lasts::ForNow
            }
        }
    }
}

/// Whether calling again could change the core's `error`.
pub(crate) fn control_lasts(error: &ControlError) -> Lasts {
    if error.is_unknown_session() {
        return Lasts::ForGood;
    }
    match error {
        ControlError::Refused(_) => Lasts::ForGood,
        ControlError::UnknownSession(_) | ControlError::Invalid(_) | ControlError::Daemon(_) => {
            Lasts::ForNow
        }
    }
}

/// The server a call acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// This server's own daemon.
    Local,
    Remote(Remote),
}

/// Where a call goes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Route {
    One(Target),
    /// `list` with no `server`: this server and every remote.
    Every,
}

impl Target {
    /// Runs `action` with `arguments` for `caller` on this server.
    pub(crate) async fn call(
        &self,
        control: &SessionControl,
        caller: &Caller,
        action: Action,
        arguments: Map<String, Value>,
    ) -> Result<Value, RouteError> {
        match self {
            Self::Local => Ok(control.call(caller, action, arguments).await?),
            Self::Remote(remote) => {
                caller.grant.permit(action).map_err(ControlError::from)?;
                Ok(call(remote, action, arguments, &forwarded(caller), Asked::ByCall).await?)
            }
        }
    }

    /// Refuses `caller` a follow of `session` it may not `wait` for. A
    /// remote checks as its `wait` does, on [`Self::now`].
    pub(crate) async fn check_follow(
        &self,
        control: &SessionControl,
        caller: &Caller,
        session: &SessionId,
    ) -> Result<(), RouteError> {
        match self {
            Self::Local => Ok(control
                .check_follow(caller, session, DaemonUse::Call)
                .await?),
            Self::Remote(_) => Ok(caller
                .grant
                .permit(Action::Wait)
                .map_err(ControlError::from)?),
        }
    }

    /// Where `session` is now, as a `wait` that does not wait reports it.
    pub(crate) async fn now(
        &self,
        control: &SessionControl,
        caller: &Caller,
        session: &SessionId,
    ) -> Result<WaitResult, RouteError> {
        match self {
            Self::Local => Ok(control.now(session).await?),
            Self::Remote(remote) => {
                Ok(wait(
                    remote,
                    session,
                    /*timeout_secs*/ 0,
                    /*cursor*/ None,
                    &forwarded(caller),
                    Asked::ByCall,
                )
                .await?)
            }
        }
    }

    /// The remote's name, or `None` for this server.
    pub(crate) fn remote_name(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Remote(remote) => Some(&remote.name),
        }
    }
}

/// The context a call for a remote carries there: the caller's depth and
/// access. Its workspaces are paths on this machine, which name nothing on
/// another, so the remote's token for the hub bounds them there instead.
fn forwarded(caller: &Caller) -> CallerContext {
    CallerContext {
        depth: caller.depth,
        scope: Scope {
            access: caller.grant.scope.access,
            workspaces: Workspaces::Any,
        },
    }
}

/// How a call arrived: from a host, or forwarded by a hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Arrival {
    Direct,
    /// Served on this machine's sessions, whatever remotes it has.
    Forwarded(Asked),
}

impl Arrival {
    /// How the request whose `_meta` is `meta` arrived.
    pub(crate) fn of(meta: &Map<String, Value>) -> Self {
        meta.get(FORWARDED)
            .and_then(Value::as_str)
            .and_then(Asked::from_name)
            .map_or(Self::Direct, Self::Forwarded)
    }
}

/// This server's name, and how it reads its remotes.
#[derive(Clone, Debug)]
pub(crate) struct Servers {
    local: String,
    /// Whether a failure to read the remotes was logged already: a store a
    /// stdio host's environment cannot reach fails every call alike.
    warned: Arc<AtomicBool>,
}

impl Servers {
    /// A server called `local`, as `server` names it.
    pub(crate) fn new(local: String) -> Self {
        Self {
            local,
            warned: Arc::default(),
        }
    }

    /// The machines a call that arrived as `arrival` may name, as they
    /// are now: none but this one for a forwarded call.
    pub(crate) async fn load(&self, arrival: Arrival) -> Machines {
        let remotes = match arrival {
            Arrival::Forwarded(_) => Ok(RemoteSet::default()),
            Arrival::Direct => token_store::load_fresh::<RemoteSet>()
                .await
                .map_err(|error| {
                    let error = format!("{error:#}");
                    if !self.warned.swap(true, Ordering::Relaxed) {
                        tracing::warn!(%error, "could not read the MCP server remotes; serving this machine only");
                    }
                    error
                }),
        };
        Machines {
            local: self.local.clone(),
            remotes,
        }
    }
}

/// The machines one call may name: this one, and the remotes as the call
/// read them, or why they could not be read. A server whose remotes cannot
/// be read serves its own sessions, as one with none does.
#[derive(Debug)]
pub(crate) struct Machines {
    local: String,
    remotes: Result<RemoteSet, String>,
}

/// How a tool takes `server` on a hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Naming {
    /// It acts on one machine's session: `server` is required.
    One,
    /// `list`: no `server` lists every machine.
    Every,
    /// `unwatch`: it forgets a watch this server keeps, wherever the
    /// session is, so `server` is optional.
    Here,
}

impl Naming {
    fn of(tool: ServedTool) -> Self {
        match tool {
            ServedTool::Core(Action::List) => Self::Every,
            ServedTool::Unwatch => Self::Here,
            ServedTool::Core(
                Action::Read
                | Action::New
                | Action::Prompt
                | Action::Steer
                | Action::Cancel
                | Action::Wait
                // Never served (`tool::served_actions`); named for the match
                // to stay exhaustive over the core's actions.
                | Action::Answer,
            )
            | ServedTool::Watch => Self::One,
        }
    }
}

impl Machines {
    fn remotes(&self) -> &[Remote] {
        self.remotes
            .as_ref()
            .map_or(&[], |remotes| remotes.remotes.as_slice())
    }

    /// Whether there are remotes, so every tool takes `server`.
    pub(crate) fn is_hub(&self) -> bool {
        !self.remotes().is_empty()
    }

    /// Where a call to `tool` goes, taking `server` out of its
    /// `arguments`. With no remotes, every call is this server's, and
    /// `server` may name it. With remotes, every call names its server,
    /// but for a `list` that names none, which lists every one, and an
    /// `unwatch`, which this server answers.
    pub(crate) fn route(
        &self,
        tool: ServedTool,
        arguments: &mut Map<String, Value>,
    ) -> Result<Route, RouteError> {
        let name_of_tool = tool.name();
        let named = match arguments.remove(SERVER) {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) => Some(name),
            Some(other) => {
                return Err(RouteError::Server(format!(
                    "invalid `{name_of_tool}` arguments: `{SERVER}` is a server name, not {other}"
                )))
            }
        };
        let Some(name) = named else {
            return match (self.is_hub(), Naming::of(tool)) {
                (false, _) | (true, Naming::Here) => Ok(Route::One(Target::Local)),
                (true, Naming::Every) => Ok(Route::Every),
                (true, Naming::One) => Err(RouteError::Server(format!(
                    "`{name_of_tool}` needs `{SERVER}`, the machine to act on: one of {}",
                    self.names().join(", ")
                ))),
            };
        };
        if name == self.local {
            if self
                .remotes
                .as_ref()
                .is_ok_and(|remotes| remotes.get(&name).is_some())
            {
                return Err(RouteError::Server(format!(
                    "remote {name:?} has this server's own name, so `{SERVER}` cannot tell them apart; rename the remote with `rho mcp remote add`"
                )));
            }
            return Ok(Route::One(Target::Local));
        }
        match (
            &self.remotes,
            self.remotes().iter().find(|remote| remote.name == name),
        ) {
            (_, Some(remote)) => Ok(Route::One(Target::Remote(remote.clone()))),
            (Err(error), None) => Err(RouteError::Server(format!(
                "could not read this server's remotes, so it cannot reach server {name:?}: {error}"
            ))),
            (Ok(_), None) => Err(RouteError::Server(format!(
                "no server {name:?}; servers: {}",
                self.names().join(", ")
            ))),
        }
    }

    /// Every server's name, this one's first, each once.
    fn names(&self) -> Vec<&str> {
        let mut names = vec![self.local.as_str()];
        for remote in self.remotes() {
            if !names.contains(&remote.name.as_str()) {
                names.push(&remote.name);
            }
        }
        names
    }

    /// `tool`'s input `schema` with `server`, when there are remotes to
    /// choose among.
    pub(crate) fn with_server(
        &self,
        tool: ServedTool,
        mut schema: Map<String, Value>,
    ) -> Map<String, Value> {
        if !self.is_hub() {
            return schema;
        }
        let naming = Naming::of(tool);
        let description = match naming {
            Naming::One => {
                "The machine the session is on, or to start it on. Webhook news names it as `server`."
            }
            Naming::Every => "The machine whose sessions to list; omit it to list every machine's.",
            Naming::Here => "The machine the session is on; this server keeps every watch alike.",
        };
        if let Some(Value::Object(properties)) = schema.get_mut("properties") {
            properties.insert(
                SERVER.to_owned(),
                serde_json::json!({
                    "type": "string",
                    "enum": self.names(),
                    "description": description,
                }),
            );
        }
        if naming == Naming::One {
            match schema.get_mut("required") {
                Some(Value::Array(required)) => required.push(Value::from(SERVER)),
                _ => {
                    schema.insert("required".to_owned(), serde_json::json!([SERVER]));
                }
            }
        }
        schema
    }

    /// `list` on this server and on every remote at once: each one's
    /// sessions, or why it could not list them, by server.
    pub(crate) async fn list_every(
        &self,
        control: &SessionControl,
        caller: &Caller,
        arguments: Map<String, Value>,
    ) -> Value {
        let targets: Vec<(&str, Target)> = std::iter::once((self.local.as_str(), Target::Local))
            .chain(
                self.remotes()
                    .iter()
                    .map(|remote| (remote.name.as_str(), Target::Remote(remote.clone()))),
            )
            .collect();
        let lists = join_all(
            targets
                .iter()
                .map(|(_, target)| target.call(control, caller, Action::List, arguments.clone())),
        )
        .await;
        let servers = targets
            .iter()
            .zip(lists)
            .map(|((name, _), listed)| {
                let mut entry = Map::from_iter([(SERVER.to_owned(), Value::from(*name))]);
                match listed {
                    Ok(Value::Object(listed)) => entry.extend(listed),
                    Ok(other) => {
                        entry.insert(
                            "error".to_owned(),
                            Value::from(format!(
                                "`list` answered something other than an object: {other}"
                            )),
                        );
                    }
                    Err(error) => {
                        entry.insert("error".to_owned(), Value::from(error.to_string()));
                    }
                }
                Value::Object(entry)
            })
            .collect();
        serde_json::json!({ "servers": Value::Array(servers) })
    }

    /// The name `target` goes by.
    pub(crate) fn name_of<'a>(&'a self, target: &'a Target) -> &'a str {
        target.remote_name().unwrap_or(&self.local)
    }

    /// Names the server a call to `tool` acted on in its `result`, on a
    /// hub, so the host can keep track of where each session is: all but
    /// `unwatch`, which this server answers wherever the session is.
    pub(crate) fn stamp(&self, result: &mut Value, target: &Target, tool: ServedTool) {
        if !self.is_hub() || Naming::of(tool) == Naming::Here {
            return;
        }
        if let Value::Object(result) = result {
            result.insert(SERVER.to_owned(), Value::from(self.name_of(target)));
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
