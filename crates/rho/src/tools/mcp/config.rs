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

#[derive(Clone, Debug, Serialize)]
pub(crate) struct McpServerConfig {
    pub(crate) enabled: bool,
    pub(crate) tools: McpToolFilter,
    #[serde(flatten)]
    pub(crate) transport: McpTransport,
}

impl<'de> Deserialize<'de> for McpServerConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum TransportKind {
            Stdio,
            StreamableHttp,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawServer {
            #[serde(default = "enabled_by_default")]
            enabled: bool,
            #[serde(default)]
            tools: McpToolFilter,
            transport: TransportKind,
            command: Option<String>,
            args: Option<Vec<String>>,
            cwd: Option<PathBuf>,
            env: Option<BTreeMap<String, String>>,
            env_from_env: Option<BTreeMap<String, String>>,
            url: Option<String>,
            headers_from_env: Option<BTreeMap<String, String>>,
        }

        let raw = RawServer::deserialize(deserializer)?;
        let transport = match raw.transport {
            TransportKind::Stdio => {
                if raw.url.is_some() || raw.headers_from_env.is_some() {
                    return Err(serde::de::Error::custom(
                        "stdio server cannot set HTTP fields",
                    ));
                }
                let command = raw
                    .command
                    .ok_or_else(|| serde::de::Error::missing_field("command"))?;
                if command.trim().is_empty() {
                    return Err(serde::de::Error::custom("stdio command must not be empty"));
                }
                McpTransport::Stdio {
                    command,
                    args: raw.args.unwrap_or_default(),
                    cwd: raw.cwd,
                    env: raw.env.unwrap_or_default(),
                    env_from_env: raw.env_from_env.unwrap_or_default(),
                }
            }
            TransportKind::StreamableHttp => {
                if raw.command.is_some()
                    || raw.args.is_some()
                    || raw.cwd.is_some()
                    || raw.env.is_some()
                    || raw.env_from_env.is_some()
                {
                    return Err(serde::de::Error::custom(
                        "Streamable HTTP server cannot set stdio fields",
                    ));
                }
                let url = raw
                    .url
                    .ok_or_else(|| serde::de::Error::missing_field("url"))?;
                super::validate_remote_url(&url).map_err(serde::de::Error::custom)?;
                McpTransport::StreamableHttp {
                    url,
                    headers_from_env: raw.headers_from_env.unwrap_or_default(),
                }
            }
        };
        Ok(Self {
            enabled: raw.enabled,
            tools: raw.tools,
            transport,
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
        /// Header names mapped to environment variable names. Values never live in config.
        headers_from_env: BTreeMap<String, String>,
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
