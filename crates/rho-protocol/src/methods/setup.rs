//! A session's setup: the choices a client makes for a session the daemon
//! opens for it, the ones `rho acp` takes per session and `rho` takes as
//! flags: directories outside the workspace, MCP servers of the client's
//! own, and parts of the session left out. [`super::SessionNewParams`] and
//! [`super::SessionAttachParams`] carry one; the daemon applies it only when
//! that request opens the session.

use std::{collections::BTreeSet, fmt, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Choices a client makes for a session the daemon opens for it. The
/// session keeps them until the daemon unloads it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSetup {
    /// Directories outside the workspace the session's tools may use, as
    /// `--add-dir` adds them: absolute paths to existing directories. An
    /// entry that is not one is [`crate::ErrorCode::InvalidParams`], naming
    /// it, and the session does not open. One the workspace or another
    /// entry already holds is folded in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<PathBuf>,
    /// MCP servers the client provides, which the session connects next to
    /// those in config; one replaces a config server of the same name. An
    /// entry the daemon cannot connect safely, such as plain HTTP to a host
    /// other than this machine, is not connected, yet still replaces the
    /// config server of its name. When a name repeats, the last entry wins.
    /// These servers may not sample the user's model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServer>,
    /// Parts of the session left out, as `rho`'s `--no-tools`,
    /// `--no-system-prompt`, and `--no-subagents` leave them out, and
    /// questions a client cannot show.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub without: BTreeSet<SessionPart>,
}

impl SessionSetup {
    /// Whether this asks for nothing, so the wire leaves it out.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A part of a session a client can leave out.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionPart {
    /// The tools: the model is offered none, as with `--no-tools`.
    Tools,
    /// Rho's system prompt, with the workspace's `AGENTS.md` and skills, as
    /// with `--no-system-prompt`.
    SystemPrompt,
    /// The tools that start subagents, as with `--no-subagents`.
    Subagents,
    /// Questions to the user: the session has no questionnaire tool, and an
    /// MCP server's question is declined, for a client that cannot show
    /// [`crate::Ask::Question`].
    Questions,
}

/// An MCP server a client provides, by how the daemon reaches it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "transport", rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpServer {
    /// A process the daemon starts and talks to over its stdin and stdout.
    Stdio {
        /// The server's name, which its tools are listed under.
        name: String,
        /// The program to run.
        command: PathBuf,
        args: Vec<String>,
        /// Variables the process gets on top of the session's environment;
        /// a name may appear once.
        env: Vec<EnvVariable>,
    },
    /// A server the daemon reaches over Streamable HTTP: HTTPS, or plain
    /// HTTP only to this machine.
    Http {
        /// The server's name, which its tools are listed under.
        name: String,
        url: String,
        /// Headers every request to the server carries; a name may appear
        /// once.
        headers: Vec<HttpHeader>,
    },
}

/// An environment variable an MCP server's process gets. The value is a
/// secret: the daemon never logs it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvVariable {
    pub name: String,
    pub value: String,
}

/// Shows only the name: the value is a secret.
impl fmt::Debug for EnvVariable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnvVariable")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// A header every request to an MCP server carries, such as its
/// `Authorization`. The value is a secret: the daemon never logs it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

/// Shows only the name: the value is a secret.
impl fmt::Debug for HttpHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpHeader")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}
