//! A session's setup as the daemon opens it with: the opening request's
//! [`SessionSetup`] on top of what a reopened session's file keeps,
//! checked and converted once (#1661).
//!
//! The session owns its added directories and the parts it leaves out but
//! questions: its file keeps them, and a reopen adds the request's to the
//! saved ones, so a later client can narrow a session further but never
//! widen it, and a reopen never drops a saved directory that still exists.
//! That holds across daemon reopens only: the TUI keeps an existing record
//! but does not record its own `--no-*` flags, until it becomes a daemon
//! client (#1667).
//! A new session opened without saving has no file, so none of this is kept
//! and no request can reopen it.
//! The opening client owns its MCP servers, whose `env` and `headers` may
//! hold secrets, whether it shows questions, and whether its own threads
//! run the session's children, which goes with the MCP server they go
//! through: a reopen takes them from its own request alone, so a prompt,
//! which brings no setup, reopens a session without them.

use std::path::Path;

use rho_protocol::{ChildThreads, ErrorCode, IdleWake, RpcError, SessionPart, SessionSetup};

use super::open_failed;
use crate::{
    added_dirs::{session_added_dirs, AddedDirs},
    app::{
        host_mcp::host_mcp_config,
        session_runtime::{ChildAgents, IdleTurns, LeftOut, QuestionSupport},
    },
    session::{Session as StoredSession, SessionPersistence},
    tools::mcp::config::McpConfig,
};

/// What a session opens with.
pub(super) struct Setup {
    /// The parts it leaves out: those its file keeps, and the request's.
    pub(super) left_out: LeftOut,
    /// Whether its client shows questions.
    pub(super) questions: QuestionSupport,
    /// Who runs its children: the host's threads when the request says so.
    pub(super) children: ChildAgents,
    /// Whether the daemon starts a turn when its background work finishes
    /// idle: when the request says so.
    pub(super) idle_turns: IdleTurns,
    /// The directories it adds: those its file keeps that still exist, and
    /// the request's.
    pub(super) added_dirs: AddedDirs,
    /// The opening client's MCP servers.
    pub(super) host_mcp: McpConfig,
    /// Whether it has a file: not when the request leaves saving out.
    pub(super) persistence: SessionPersistence,
    /// Whether the request adds to what the session's file keeps, so a
    /// reopen saves at once rather than lose it to an unload before the
    /// next turn saves.
    pub(super) adds_to_saved: bool,
}

impl Setup {
    /// The setup of the session in workspace `cwd` that `requested` opens,
    /// on top of what `saved`, the session's file, keeps when the request
    /// reopens one. A directory the session cannot grant and a part this
    /// daemon does not know are invalid params; a workspace or saved record
    /// that cannot be read fails the open.
    pub(super) fn resolve(
        requested: &SessionSetup,
        cwd: &Path,
        saved: Option<&StoredSession>,
    ) -> Result<Self, RpcError> {
        let root = std::fs::canonicalize(cwd).map_err(|error| {
            open_failed(anyhow::anyhow!(
                "could not resolve the workspace {}: {error}",
                cwd.display()
            ))
        })?;
        let (saved_left_out, saved_dirs) = match saved {
            Some(stored) => {
                let restored = AddedDirs::from_storage(stored, &root);
                for dir in &restored.missing {
                    tracing::warn!(
                        session = stored.id(),
                        dir = %dir.display(),
                        "could not restore added directory: not found"
                    );
                }
                (LeftOut::saved(stored).map_err(open_failed)?, restored.dirs)
            }
            None => (LeftOut::default(), AddedDirs::default()),
        };
        let mut asked = LeftOut::default();
        let mut questions = QuestionSupport::Available;
        let mut persistence = SessionPersistence::Saved;
        for part in &requested.without {
            match part {
                SessionPart::Tools => asked.tools = true,
                SessionPart::SystemPrompt => asked.system_prompt = true,
                SessionPart::Subagents => asked.subagents = true,
                SessionPart::Questions => questions = QuestionSupport::Unavailable,
                SessionPart::Saving if saved.is_some() => {
                    return Err(invalid(
                        "a saved session cannot be reopened without saving; open a new one",
                    ))
                }
                SessionPart::Saving => persistence = SessionPersistence::Ephemeral,
                // `SessionPart` is `#[non_exhaustive]`, so the arm is
                // required: a part from a later protocol minor that this
                // daemon does not know fails here as invalid params.
                _ => return Err(invalid("unknown session part")),
            }
        }
        let asked_dirs = session_added_dirs(&requested.additional_directories, &root)
            .map_err(|rejected| invalid(rejected.to_string()))?;
        let children = match requested.child_threads {
            ChildThreads::Daemon => ChildAgents::InProcess,
            ChildThreads::Host => ChildAgents::HostThreads,
        };
        let idle_turns = match requested.idle_wake {
            IdleWake::Never => IdleTurns::Never,
            IdleWake::Turn => IdleTurns::Host,
        };
        let left_out = saved_left_out.union(asked);
        let added_dirs = saved_dirs.union(&root, &asked_dirs);
        Ok(Self {
            adds_to_saved: saved.is_some()
                && (left_out != saved_left_out || added_dirs != saved_dirs),
            left_out,
            questions,
            children,
            idle_turns,
            added_dirs,
            persistence,
            host_mcp: host_mcp_config(&requested.mcp_servers),
        })
    }
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, message)
}
