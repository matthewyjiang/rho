//! Translate a plugin `mcp.json` into the generic native MCP configuration.

use std::{collections::BTreeMap, path::Path};

use serde::Deserialize;
use serde_json::Value;

use super::{contain, manifest::optional_non_null};
use crate::tools::mcp::config::{
    InvalidMcpServer, McpFilesystemPolicy, McpServerConfig, McpToolFilter, McpTransport,
};

pub(crate) const MCP_SCHEMA_1_0_0: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

const PLUGIN_ROOT_PLACEHOLDER: &str = "${PLUGIN_ROOT}";
const PLUGIN_DATA_PLACEHOLDER: &str = "${PLUGIN_DATA}";
const PLUGIN_ROOT_PREFIX: &str = "${PLUGIN_ROOT}/";
const PLUGIN_DATA_PREFIX: &str = "${PLUGIN_DATA}/";

#[derive(Debug, Default)]
pub(crate) struct PluginMcpOutcome {
    pub(crate) servers: Vec<(String, McpServerConfig)>,
    pub(crate) invalid: Vec<InvalidMcpServer>,
    pub(crate) skipped_unsupported: Vec<String>,
    pub(crate) disabled_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDocument {
    #[serde(rename = "$schema")]
    schema: String,
    #[serde(rename = "mcpServers")]
    servers: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum RawServer {
    #[serde(rename = "stdio")]
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default, deserialize_with = "optional_non_null")]
        cwd: Option<String>,
    },
    #[serde(rename = "streamable-http")]
    StreamableHttp {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

#[derive(Clone, Copy)]
struct PluginPaths<'a> {
    root: &'a Path,
    storage_root: &'a Path,
    data_dir: &'a Path,
}

enum ServerRejection {
    Invalid(String),
    UnsupportedTransport(&'static str),
}

pub(crate) fn load_plugin_mcp(
    text: &str,
    plugin_name: &str,
    root: &Path,
    storage_root: &Path,
    data_dir: &Path,
) -> PluginMcpOutcome {
    let document: RawDocument = match serde_json::from_str(text) {
        Ok(document) => document,
        Err(error) => {
            return PluginMcpOutcome {
                disabled_reason: Some(error.to_string()),
                ..PluginMcpOutcome::default()
            };
        }
    };
    if document.schema != MCP_SCHEMA_1_0_0 {
        return PluginMcpOutcome {
            disabled_reason: Some(format!(
                "unsupported Agent Plugins schema `{}`",
                document.schema
            )),
            ..PluginMcpOutcome::default()
        };
    }

    let paths = PluginPaths {
        root,
        storage_root,
        data_dir,
    };
    let mut outcome = PluginMcpOutcome::default();
    for (name, value) in document.servers {
        if name.is_empty() {
            outcome.invalid.push(InvalidMcpServer {
                identity: format!("{plugin_name}/"),
                error: "server name must not be empty".to_string(),
            });
            continue;
        }
        match translate_server(&name, value, paths) {
            Ok(config) => outcome.servers.push((name, config)),
            Err(ServerRejection::Invalid(error)) => outcome.invalid.push(InvalidMcpServer {
                identity: format!("{plugin_name}/{name}"),
                error,
            }),
            Err(ServerRejection::UnsupportedTransport(transport)) => outcome
                .skipped_unsupported
                .push(format!("{plugin_name}/{name} ({transport})")),
        }
    }
    outcome
}

fn translate_server(
    name: &str,
    value: Value,
    paths: PluginPaths<'_>,
) -> Result<McpServerConfig, ServerRejection> {
    if value.get("type").and_then(Value::as_str) == Some("sse") {
        return Err(ServerRejection::UnsupportedTransport("sse"));
    }
    let raw: RawServer = serde_json::from_value(value)
        .map_err(|error| ServerRejection::Invalid(format!("server `{name}`: {error}")))?;
    let (transport, filesystem) = match raw {
        RawServer::Stdio {
            command,
            args,
            env,
            cwd,
        } => {
            let (transport, filesystem) =
                translate_stdio(name, command, args, env, cwd.as_deref(), paths)?;
            (transport, Some(filesystem))
        }
        RawServer::StreamableHttp { url, headers } => (translate_remote(name, url, headers)?, None),
    };
    Ok(McpServerConfig {
        enabled: true,
        tools: McpToolFilter::default(),
        transport,
        filesystem,
    })
}

fn translate_stdio(
    name: &str,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: Option<&str>,
    paths: PluginPaths<'_>,
) -> Result<(McpTransport, McpFilesystemPolicy), ServerRejection> {
    let invalid = |error: String| ServerRejection::Invalid(error);
    if command.trim().is_empty() {
        return Err(invalid(format!(
            "server `{name}` requires a non-empty `command`"
        )));
    }
    let command = if command.contains('/') || command.contains('\\') {
        let tail = command.strip_prefix("./").ok_or_else(|| {
            invalid(format!(
                "server `{name}` command must be a bare executable name or start with ./"
            ))
        })?;
        contain::resolve_in_root(paths.root, tail)
            .map_err(|error| invalid(format!("server `{name}`: {error}")))?
            .to_string_lossy()
            .to_string()
    } else {
        command
    };

    let root_display = paths.root.to_string_lossy();
    let data_display = paths.data_dir.to_string_lossy();
    let args = args
        .into_iter()
        .map(|arg| expand_placeholders(&arg, &root_display, &data_display))
        .collect();
    let mut expanded_env = BTreeMap::new();
    for (key, value) in env {
        if key == "PLUGIN_ROOT" || key == "PLUGIN_DATA" {
            return Err(invalid(format!(
                "server `{name}` `env` must not set reserved variable `{key}`"
            )));
        }
        #[cfg(windows)]
        if key.eq_ignore_ascii_case("PLUGIN_ROOT") || key.eq_ignore_ascii_case("PLUGIN_DATA") {
            continue;
        }
        expanded_env.insert(
            key,
            expand_placeholders(&value, &root_display, &data_display),
        );
    }
    expanded_env.insert("PLUGIN_ROOT".to_string(), root_display.into_owned());
    expanded_env.insert("PLUGIN_DATA".to_string(), data_display.into_owned());

    let cwd = cwd
        .map(|cwd| resolve_cwd(name, cwd, paths))
        .transpose()?
        .unwrap_or_else(|| paths.root.to_path_buf());
    let data_relative_to_storage =
        paths
            .data_dir
            .strip_prefix(paths.storage_root)
            .map_err(|_| {
                invalid(format!(
                    "server `{name}` PLUGIN_DATA is outside the plugin storage root"
                ))
            })?;
    Ok((
        McpTransport::Stdio {
            command,
            args,
            cwd: Some(cwd),
            env: expanded_env,
            env_from_env: BTreeMap::new(),
        },
        McpFilesystemPolicy {
            directory_root: paths.storage_root.to_path_buf(),
            directory_relative_to_root: data_relative_to_storage.to_path_buf(),
            allowed_roots: vec![paths.root.to_path_buf(), paths.data_dir.to_path_buf()],
        },
    ))
}

fn resolve_cwd(
    name: &str,
    cwd: &str,
    paths: PluginPaths<'_>,
) -> Result<std::path::PathBuf, ServerRejection> {
    let resolve = |base: &Path, tail: &str| {
        contain::resolve_in_root(base, tail)
            .map_err(|error| ServerRejection::Invalid(format!("server `{name}` cwd: {error}")))
    };
    if let Some(tail) = cwd.strip_prefix("./") {
        return resolve(paths.root, tail);
    }
    if cwd == PLUGIN_ROOT_PLACEHOLDER {
        return Ok(paths.root.to_path_buf());
    }
    if let Some(tail) = cwd.strip_prefix(PLUGIN_ROOT_PREFIX) {
        return resolve(paths.root, tail);
    }
    if cwd == PLUGIN_DATA_PLACEHOLDER {
        return Ok(paths.data_dir.to_path_buf());
    }
    if let Some(tail) = cwd.strip_prefix(PLUGIN_DATA_PREFIX) {
        let relative = paths
            .data_dir
            .strip_prefix(paths.storage_root)
            .map_err(|_| {
                ServerRejection::Invalid(format!(
                    "server `{name}` PLUGIN_DATA is outside the plugin storage root"
                ))
            })?
            .join(tail);
        return resolve(
            paths.storage_root,
            relative.to_str().ok_or_else(|| {
                ServerRejection::Invalid(format!("server `{name}` cwd is not valid UTF-8"))
            })?,
        );
    }
    Err(ServerRejection::Invalid(format!(
        "server `{name}` cwd must be plugin-relative, ${{PLUGIN_ROOT}}-rooted, or ${{PLUGIN_DATA}}-rooted"
    )))
}

fn translate_remote(
    name: &str,
    url: String,
    headers: BTreeMap<String, String>,
) -> Result<McpTransport, ServerRejection> {
    let invalid =
        |error: anyhow::Error| ServerRejection::Invalid(format!("server `{name}`: {error}"));
    let parsed = crate::tools::mcp::parse_remote_url(&url).map_err(invalid)?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ServerRejection::Invalid(format!(
            "server `{name}` URL must not contain user information"
        )));
    }
    if parsed.fragment().is_some() {
        return Err(ServerRejection::Invalid(format!(
            "server `{name}` URL must not contain a fragment"
        )));
    }
    crate::tools::mcp::validate_literal_headers(&headers).map_err(invalid)?;
    Ok(McpTransport::StreamableHttp {
        url,
        headers,
        headers_from_env: BTreeMap::new(),
    })
}

/// Single-pass, non-recursive expansion of `${PLUGIN_ROOT}` and
/// `${PLUGIN_DATA}`. Replacement text is never rescanned.
pub(crate) fn expand_placeholders(input: &str, plugin_root: &str, plugin_data: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let candidate = &rest[start..];
        if let Some(after) = candidate.strip_prefix(PLUGIN_ROOT_PLACEHOLDER) {
            output.push_str(plugin_root);
            rest = after;
        } else if let Some(after) = candidate.strip_prefix(PLUGIN_DATA_PLACEHOLDER) {
            output.push_str(plugin_data);
            rest = after;
        } else {
            output.push_str("${");
            rest = &candidate[2..];
        }
    }
    output.push_str(rest);
    output
}
