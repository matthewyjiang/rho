//! A session's setup: the choices a client makes for a session the daemon
//! opens for it, the ones `rho acp` takes per session and `rho` takes as
//! flags: directories outside the workspace, MCP servers of the client's
//! own, and parts of the session left out. [`super::SessionNewParams`] and
//! [`super::SessionAttachParams`] carry one; the daemon applies it only when
//! that request opens the session.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::PathBuf,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Choices a client makes for a session the daemon opens for it.
///
/// The session owns its directories and the parts it leaves out but
/// questions: it saves them, and a reopen adds the opening request's to the
/// saved ones, so a later client can narrow a session, never widen it. The
/// opening client owns its MCP servers and whether it shows questions: a
/// reopened session has those its opening request names, and none when a
/// prompt reopens it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSetup {
    /// Directories outside the workspace the session's tools may use, as
    /// `--add-dir` adds them: absolute paths to existing directories. An
    /// entry that is not one is [`crate::ErrorCode::InvalidParams`], naming
    /// it, and the session does not open. One the workspace or another
    /// entry already holds is folded in. The session saves them; a reopen
    /// keeps the saved ones that still exist and adds these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<PathBuf>,
    /// MCP servers the client provides, which the session connects next to
    /// those in config; one replaces a config server of the same name. An
    /// entry the daemon cannot connect safely, such as plain HTTP to a host
    /// other than this machine, is not connected, yet still replaces the
    /// config server of its name. When a name repeats, the last entry wins.
    /// These servers may not sample the user's model. The session does not
    /// save them, since their `env` and `headers` may hold secrets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServer>,
    /// Parts of the session left out, as `rho`'s `--no-tools`,
    /// `--no-system-prompt`, and `--no-subagents` leave them out, and
    /// questions a client cannot show. The session saves all but
    /// [`SessionPart::Questions`]; a reopen leaves out the saved parts and
    /// these.
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
        /// The program's arguments, after its name.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        /// Variables the process gets on top of the session's environment,
        /// by name.
        #[serde(default, skip_serializing_if = "SecretMap::is_empty")]
        env: SecretMap,
    },
    /// A server the daemon reaches over Streamable HTTP: HTTPS, or plain
    /// HTTP only to this machine.
    Http {
        /// The server's name, which its tools are listed under.
        name: String,
        /// The server's endpoint, such as `https://example.com/mcp`.
        url: String,
        /// Headers every request to the server carries, such as its
        /// `Authorization`, by name.
        #[serde(default, skip_serializing_if = "SecretMap::is_empty")]
        headers: SecretMap,
    },
}

/// Values by name that may be secrets, such as an MCP server's environment
/// variables and headers. Its `Debug` shows only the names, so the daemon
/// never logs a value.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct SecretMap(BTreeMap<String, String>);

impl SecretMap {
    /// Whether this holds no values, so the wire leaves it out.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every value, by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }
}

impl FromIterator<(String, String)> for SecretMap {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(values: I) -> Self {
        Self(values.into_iter().collect())
    }
}

impl IntoIterator for SecretMap {
    type Item = (String, String);
    type IntoIter = std::collections::btree_map::IntoIter<String, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Shows only the names: the values may be secrets.
impl fmt::Debug for SecretMap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_set().entries(self.0.keys()).finish()
    }
}
