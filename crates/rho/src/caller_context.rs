//! Where a session sits among the sessions that started it, and the most
//! the call that started its turn may let it do (#1765).
//!
//! One [`CallerContext`] travels the whole chain:
//! - the session-control core (`app::session_control`) writes it into the
//!   environment of every session it starts or prompts, one level deeper and
//!   with the caller's scope, and the session's processes inherit it, so a
//!   `rho mcp serve` a session's shell runs reads it from its environment;
//! - rho's MCP client sends the calling session's current context with every
//!   call to a server that accepts it ([`ACCEPTS_KEY`]), so a long-running
//!   server, such as a stdio `rho mcp serve` started when the session loaded,
//!   or one reached over HTTP, answers for the turn that calls it, not for the
//!   one that was current when it started.
//!
//! The depth guards against runaway recursion and the scope keeps a grant's
//! limits on the sessions it starts. Neither is a sandbox: a session with a
//! shell can reach the daemon's socket directly.

use std::{ffi::OsString, path::PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// The variable that carries a session's nesting depth to the processes it
/// starts. A session a person starts has none, depth 0.
pub(crate) const SESSION_DEPTH_ENV: &str = "RHO_SESSION_DEPTH";

/// The variable that carries a session's [`Scope`], as JSON, to the processes
/// it starts. Absent when the scope is [`Scope::FULL`].
pub(crate) const SESSION_SCOPE_ENV: &str = "RHO_SESSION_SCOPE";

/// Both variables, for code that forwards or clears them together.
pub(crate) const ENV_NAMES: [&str; 2] = [SESSION_DEPTH_ENV, SESSION_SCOPE_ENV];

/// The key of the context in a request's `_meta`, and of the experimental
/// capability a server declares to say it reads it. A client sends the
/// context only to a server that declares it, so other servers never see a
/// session's workspaces.
pub(crate) const ACCEPTS_KEY: &str = "rho/caller";

/// What a grant lets its holder do to sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Access {
    /// List, read, and wait for sessions.
    Read,
    /// Also start, prompt, steer, and cancel them.
    Write,
}

impl Access {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// The workspaces a grant reaches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Workspaces {
    Any,
    /// These directories and everything under them, as canonical paths.
    Only(Vec<PathBuf>),
}

impl Workspaces {
    /// Whether `workspace`, a canonical path, is one the grant reaches.
    pub(crate) fn allows(&self, workspace: &std::path::Path) -> bool {
        match self {
            Self::Any => true,
            Self::Only(allowed) => allowed.iter().any(|root| workspace.starts_with(root)),
        }
    }

    /// The workspaces both reach: of two nested roots, the deeper one.
    fn intersect(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Any, reached) | (reached, Self::Any) => reached.clone(),
            (Self::Only(left), Self::Only(right)) => {
                let mut both: Vec<PathBuf> = left
                    .iter()
                    .filter(|root| other.allows(root))
                    .chain(right.iter().filter(|root| self.allows(root)))
                    .cloned()
                    .collect();
                both.sort();
                both.dedup();
                Self::Only(both)
            }
        }
    }
}

/// The most a caller may do through session control: its access and the
/// workspaces it reaches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Scope {
    pub(crate) access: Access,
    pub(crate) workspaces: Workspaces,
}

impl Scope {
    /// No limit: a person's own authority over their sessions.
    pub(crate) const FULL: Self = Self {
        access: Access::Write,
        workspaces: Workspaces::Any,
    };

    /// What both scopes allow, so a scope passed down only ever narrows.
    pub(crate) fn intersect(&self, other: &Self) -> Self {
        Self {
            access: self.access.min(other.access),
            workspaces: self.workspaces.intersect(&other.workspaces),
        }
    }
}

impl Default for Scope {
    fn default() -> Self {
        Self::FULL
    }
}

/// A calling session's place and scope: depth 0 and [`Scope::FULL`] for a
/// person, or for a process no session started.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CallerContext {
    pub(crate) depth: u32,
    pub(crate) scope: Scope,
}

impl CallerContext {
    /// The context in an environment, read through `read`: a process's own,
    /// or a session's. A variable that is set but does not parse is an
    /// error, never the unlimited default.
    pub(crate) fn from_env(read: impl Fn(&str) -> Option<OsString>) -> anyhow::Result<Self> {
        let text = |name: &str| {
            read(name)
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|value| anyhow::anyhow!("{name}={value:?} is not UTF-8"))
                })
                .transpose()
        };
        let depth = match text(SESSION_DEPTH_ENV)? {
            Some(depth) => depth
                .parse()
                .with_context(|| format!("{SESSION_DEPTH_ENV}={depth:?} is not a depth"))?,
            None => 0,
        };
        let scope = match text(SESSION_SCOPE_ENV)? {
            Some(scope) => serde_json::from_str(&scope)
                .with_context(|| format!("{SESSION_SCOPE_ENV}={scope:?} is not a scope"))?,
            None => Scope::FULL,
        };
        Ok(Self { depth, scope })
    }

    /// The variables that carry this context to a child: the depth always,
    /// the scope unless it is [`Scope::FULL`].
    pub(crate) fn env(&self) -> Vec<(&'static str, String)> {
        let mut variables = vec![(SESSION_DEPTH_ENV, self.depth.to_string())];
        if self.scope != Scope::FULL {
            let scope = serde_json::to_string(&self.scope)
                .expect("a scope is plain data and always encodes");
            variables.push((SESSION_SCOPE_ENV, scope));
        }
        variables
    }

    /// The context a request's `_meta` carries under [`ACCEPTS_KEY`], if it
    /// carries one.
    pub(crate) fn from_meta(
        meta: &serde_json::Map<String, serde_json::Value>,
    ) -> anyhow::Result<Option<Self>> {
        meta.get(ACCEPTS_KEY)
            .map(|value| {
                Self::deserialize(value).with_context(|| {
                    format!("request `_meta.{ACCEPTS_KEY}` is not a caller context")
                })
            })
            .transpose()
    }

    /// A request's `_meta` carrying this context, for [`Self::from_meta`].
    pub(crate) fn to_meta(&self) -> serde_json::Map<String, serde_json::Value> {
        let value =
            serde_json::to_value(self).expect("a caller context is plain data and always encodes");
        serde_json::Map::from_iter([(ACCEPTS_KEY.to_owned(), value)])
    }
}

#[cfg(test)]
#[path = "caller_context_tests.rs"]
mod tests;
