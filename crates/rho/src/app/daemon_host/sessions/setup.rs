//! A session's setup as the daemon opens it with: the opening request's
//! [`SessionSetup`] on top of what a reopened session's file keeps,
//! checked and converted once (#1661).
//!
//! The session owns its added directories and the parts it leaves out but
//! questions: its file keeps them, and a reopen adds the request's to the
//! saved ones, so a later client can narrow a session further but never
//! widen it, and a reopen never drops a saved directory that still exists.
//! The opening client owns its MCP servers, whose `env` and `headers` may
//! hold secrets, and whether it shows questions: a reopen takes them from
//! its own request alone, so a prompt, which brings no setup, reopens a
//! session without them.

use std::path::Path;

use rho_protocol::{ErrorCode, RpcError, SessionPart, SessionSetup};

use super::open_failed;
use crate::{
    added_dirs::{session_added_dirs, AddedDirs},
    app::{
        host_mcp::host_mcp_config,
        session_runtime::{LeftOut, QuestionSupport},
    },
    session::Session as StoredSession,
    tools::mcp::config::McpConfig,
};

/// What a session opens with.
pub(super) struct Setup {
    /// The parts it leaves out: those its file keeps, and the request's.
    pub(super) left_out: LeftOut,
    /// Whether its client shows questions.
    pub(super) questions: QuestionSupport,
    /// The directories it adds: those its file keeps that still exist, and
    /// the request's.
    pub(super) added_dirs: AddedDirs,
    /// The opening client's MCP servers.
    pub(super) host_mcp: McpConfig,
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
            Some(stored) => (
                LeftOut::saved(stored).map_err(open_failed)?,
                AddedDirs::from_storage(stored, &root).dirs,
            ),
            None => (LeftOut::default(), AddedDirs::default()),
        };
        let mut asked = LeftOut::default();
        let mut questions = QuestionSupport::Available;
        for part in &requested.without {
            match part {
                SessionPart::Tools => asked.tools = true,
                SessionPart::SystemPrompt => asked.system_prompt = true,
                SessionPart::Subagents => asked.subagents = true,
                SessionPart::Questions => questions = QuestionSupport::Unavailable,
                // A part of a later protocol minor fails to decode before
                // it gets here; the arm keeps the match open to new variants.
                _ => return Err(invalid("unknown session part")),
            }
        }
        let asked_dirs = session_added_dirs(&requested.additional_directories, &root)
            .map_err(|rejected| invalid(rejected.to_string()))?;
        let left_out = saved_left_out.union(asked);
        let added_dirs = saved_dirs.union(&root, &asked_dirs);
        Ok(Self {
            adds_to_saved: saved.is_some()
                && (left_out != saved_left_out || added_dirs != saved_dirs),
            left_out,
            questions,
            added_dirs,
            host_mcp: host_mcp_config(&requested.mcp_servers),
        })
    }
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, message)
}
