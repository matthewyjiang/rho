use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct McpConfig {
    pub(crate) servers: BTreeMap<String, McpServerConfig>,
    #[serde(skip)]
    pub(crate) invalid_servers: Vec<InvalidMcpServer>,
}

impl McpConfig {
    pub(crate) fn has_enabled_servers(&self) -> bool {
        self.servers.values().any(|server| server.enabled)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// Merge source-scoped identities, preserving the existing `other`-wins
    /// behavior. Native and plugin identities use disjoint namespaces.
    pub(crate) fn merge(&mut self, other: Self) {
        for (identity, server) in other.servers {
            if self.servers.insert(identity.clone(), server).is_some() {
                tracing::warn!(server = %identity, "MCP server definition replaced during merge");
            }
        }
        self.invalid_servers.extend(other.invalid_servers);
    }
}

impl<'de> Deserialize<'de> for McpConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawConfig {
            #[serde(default)]
            servers: BTreeMap<String, toml::Value>,
        }

        let raw = RawConfig::deserialize(deserializer)?;
        let mut config = McpConfig::default();
        for (identity, value) in raw.servers {
            if let Err(error) = super::validate_identity(&identity) {
                config.invalid_servers.push(InvalidMcpServer {
                    identity,
                    error: error.to_string(),
                });
                continue;
            }
            match value.try_into::<McpServerConfig>() {
                Ok(server) => {
                    config.servers.insert(identity, server);
                }
                Err(error) => config.invalid_servers.push(InvalidMcpServer {
                    identity,
                    error: error.to_string(),
                }),
            }
        }
        Ok(config)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct InvalidMcpServer {
    pub(crate) identity: String,
    pub(crate) error: String,
}

/// Filesystem constraints attached to a package-provided MCP server.
#[derive(Clone, Debug)]
pub(crate) struct McpFilesystemPolicy {
    /// Canonical directory that owns `directory_relative_to_root`.
    pub(crate) directory_root: PathBuf,
    /// Directory to create before launch, relative to `directory_root`.
    pub(crate) directory_relative_to_root: PathBuf,
    /// Filesystem roots that absolute commands and working directories may use.
    pub(crate) allowed_roots: Vec<PathBuf>,
}

/// Severity a server should log at, mirroring the MCP `logging/setLevel`
/// levels. Kept as Rho's own enum so config never depends on the client crate's
/// type, and so an unset value means "do not send `logging/setLevel`".
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum McpLogLevel {
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Critical,
    Alert,
    Emergency,
}

// `logging` carries a SEP-2577 deprecation marker in rmcp while every
// shipping server still uses it. Rho implements the current wire protocol.
#[expect(deprecated)]
impl From<McpLogLevel> for rmcp::model::LoggingLevel {
    fn from(level: McpLogLevel) -> Self {
        match level {
            McpLogLevel::Debug => Self::Debug,
            McpLogLevel::Info => Self::Info,
            McpLogLevel::Notice => Self::Notice,
            McpLogLevel::Warning => Self::Warning,
            McpLogLevel::Error => Self::Error,
            McpLogLevel::Critical => Self::Critical,
            McpLogLevel::Alert => Self::Alert,
            McpLogLevel::Emergency => Self::Emergency,
        }
    }
}

/// Whether a server may ask Rho's model for a completion through
/// `sampling/createMessage`.
///
/// Config opt-in is only the first of two gates. Even an opted-in server must
/// still get the user's answer before each individual request runs, because a
/// server that samples in a loop spends the user's tokens. A server left at the
/// default never sees `sampling` in Rho's declared capabilities, so a
/// well-behaved server never asks.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum McpSamplingPolicy {
    /// Reject every `sampling/createMessage` and never declare the capability.
    #[default]
    Deny,
    /// The server may ask; the user allows or refuses each request.
    Ask,
}

impl McpSamplingPolicy {
    /// Whether Rho may declare `sampling` to this server.
    pub(crate) fn is_offered(self) -> bool {
        match self {
            Self::Deny => false,
            Self::Ask => true,
        }
    }
}

/// OAuth 2.1 authorization for one Streamable HTTP server.
///
/// The table's presence is the opt-in, so an empty `[mcp.servers.<id>.oauth]`
/// asks for full discovery with dynamic client registration. Nothing secret
/// belongs here: tokens live in the credential store, never in config.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct McpOAuthConfig {
    /// Client id issued out of band. Unset asks for dynamic client
    /// registration (RFC 7591) against the discovered registration endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_id: Option<String>,
    /// Scopes to request. Empty lets the server's own metadata pick them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) scopes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct McpServerConfig {
    pub(crate) enabled: bool,
    pub(crate) tools: McpToolFilter,
    /// Severity requested through `logging/setLevel` after the handshake.
    /// Unset leaves the server's own default in place.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) log_level: Option<McpLogLevel>,
    /// Whether this server may ask Rho's model for completions.
    pub(crate) sampling: McpSamplingPolicy,
    #[serde(flatten)]
    pub(crate) transport: McpTransport,
    #[serde(skip)]
    pub(crate) filesystem: Option<McpFilesystemPolicy>,
}

impl<'de> Deserialize<'de> for McpServerConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
        enum RawServer {
            Stdio {
                #[serde(default = "enabled_by_default")]
                enabled: bool,
                #[serde(default)]
                tools: McpToolFilter,
                #[serde(default)]
                log_level: Option<McpLogLevel>,
                #[serde(default)]
                sampling: McpSamplingPolicy,
                command: String,
                #[serde(default)]
                args: Vec<String>,
                cwd: Option<PathBuf>,
                #[serde(default)]
                env: BTreeMap<String, String>,
                #[serde(default)]
                env_from_env: BTreeMap<String, String>,
            },
            StreamableHttp {
                #[serde(default = "enabled_by_default")]
                enabled: bool,
                #[serde(default)]
                tools: McpToolFilter,
                #[serde(default)]
                log_level: Option<McpLogLevel>,
                #[serde(default)]
                sampling: McpSamplingPolicy,
                url: String,
                #[serde(default)]
                headers: BTreeMap<String, String>,
                #[serde(default)]
                headers_from_env: BTreeMap<String, String>,
                #[serde(default)]
                oauth: Option<McpOAuthConfig>,
            },
        }

        let (enabled, tools, log_level, sampling, transport) =
            match RawServer::deserialize(deserializer)? {
                RawServer::Stdio {
                    enabled,
                    tools,
                    log_level,
                    sampling,
                    command,
                    args,
                    cwd,
                    env,
                    env_from_env,
                } => {
                    if command.trim().is_empty() {
                        return Err(serde::de::Error::custom("stdio command must not be empty"));
                    }
                    super::validate_stdio_environment(&env, &env_from_env)
                        .map_err(serde::de::Error::custom)?;
                    (
                        enabled,
                        tools,
                        log_level,
                        sampling,
                        McpTransport::Stdio {
                            command,
                            args,
                            cwd,
                            env,
                            env_from_env,
                        },
                    )
                }
                RawServer::StreamableHttp {
                    enabled,
                    tools,
                    log_level,
                    sampling,
                    url,
                    headers,
                    headers_from_env,
                    oauth,
                } => {
                    super::parse_remote_url(&url).map_err(serde::de::Error::custom)?;
                    super::validate_literal_headers(&headers).map_err(serde::de::Error::custom)?;
                    super::validate_environment_header_names(&headers_from_env)
                        .map_err(serde::de::Error::custom)?;
                    if let Some(oauth) = &oauth {
                        super::validate_oauth_client(oauth.client_id.as_deref(), &oauth.scopes)
                            .map_err(serde::de::Error::custom)?;
                    }
                    (
                        enabled,
                        tools,
                        log_level,
                        sampling,
                        McpTransport::StreamableHttp {
                            url,
                            headers,
                            headers_from_env,
                            oauth,
                        },
                    )
                }
            };
        Ok(Self {
            enabled,
            tools,
            log_level,
            sampling,
            transport,
            filesystem: None,
        })
    }
}

const fn enabled_by_default() -> bool {
    true
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub(crate) enum McpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
        env: BTreeMap<String, String>,
        /// Child variable names mapped to ambient variable names.
        env_from_env: BTreeMap<String, String>,
    },
    StreamableHttp {
        url: String,
        /// Literal header values supplied with the configuration. Agent
        /// Plugins packages use these; plain config should prefer
        /// `headers_from_env` so secrets stay out of the file.
        headers: BTreeMap<String, String>,
        /// Header names mapped to environment variable names. Values never live in config.
        headers_from_env: BTreeMap<String, String>,
        /// OAuth 2.1 authorization, opted into by declaring the table. A
        /// configured `Authorization` header suppresses it: the user already
        /// said which credential to send.
        #[serde(skip_serializing_if = "Option::is_none")]
        oauth: Option<McpOAuthConfig>,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct McpToolFilter {
    #[serde(default)]
    pub(crate) allow: Vec<String>,
    #[serde(default)]
    pub(crate) deny: Vec<String>,
}

impl McpToolFilter {
    pub(crate) fn includes(&self, name: &str) -> bool {
        (self.allow.is_empty() || self.allow.iter().any(|allowed| allowed == name))
            && !self.deny.iter().any(|denied| denied == name)
    }
}
