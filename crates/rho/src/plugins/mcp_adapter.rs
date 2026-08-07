//! Translate a plugin `mcp.json` into the generic native MCP configuration.
//!
//! Validation is two-stage per the Agent Plugins specification: a top-level
//! failure disables MCP for the plugin, while an invalid individual server
//! disables only that entry. Valid stdio and Streamable HTTP entries become
//! ordinary `McpServerConfig` values, so the runtime from the native MCP
//! work (transport, handshake, tools/list, namespacing, permissions,
//! shutdown) is reused unchanged.

use std::{collections::BTreeMap, path::Path};

use http::{HeaderName, HeaderValue};
use serde_json::Value;

use super::contain;
use crate::tools::mcp::config::{InvalidMcpServer, McpServerConfig, McpToolFilter, McpTransport};

pub(crate) const MCP_SCHEMA_1_0_0: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

const PLUGIN_ROOT_PLACEHOLDER: &str = "${PLUGIN_ROOT}";
const PLUGIN_DATA_PLACEHOLDER: &str = "${PLUGIN_DATA}";

#[derive(Debug, Default)]
pub(crate) struct PluginMcpOutcome {
    /// Server name and its generic configuration, in manifest order.
    pub(crate) servers: Vec<(String, McpServerConfig)>,
    /// Per-server failures, identities namespaced as `<plugin>/<server>`.
    pub(crate) invalid: Vec<InvalidMcpServer>,
    /// Valid entries skipped because Rho does not support their transport.
    pub(crate) skipped_unsupported: Vec<String>,
    /// Set when the top-level document is invalid and MCP is disabled.
    pub(crate) disabled_reason: Option<String>,
}

enum ServerRejection {
    Invalid(String),
    UnsupportedTransport(&'static str),
}

pub(crate) fn load_plugin_mcp(
    text: &str,
    plugin_name: &str,
    manifest_spec_version: &str,
    root: &Path,
    data_dir: &Path,
) -> PluginMcpOutcome {
    let mut outcome = PluginMcpOutcome::default();

    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            outcome.disabled_reason = Some(format!("invalid JSON: {error}"));
            return outcome;
        }
    };
    let Some(object) = value.as_object() else {
        outcome.disabled_reason = Some("mcp.json must be a JSON object".to_string());
        return outcome;
    };
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "$schema" | "mcpServers"))
    {
        outcome.disabled_reason = Some(format!("unknown top-level field `{key}`"));
        return outcome;
    }

    let schema = match object.get("$schema").and_then(Value::as_str) {
        Some(schema) => schema,
        None => {
            outcome.disabled_reason = Some("missing or non-string required `$schema`".to_string());
            return outcome;
        }
    };
    if schema != MCP_SCHEMA_1_0_0 {
        outcome.disabled_reason = Some(format!("unsupported Agent Plugins schema `{schema}`"));
        return outcome;
    }
    if schema_version(schema) != Some(manifest_spec_version) {
        outcome.disabled_reason = Some(format!(
            "mcp.json targets Agent Plugins {:?} but plugin.json targets `{manifest_spec_version}`",
            schema_version(schema).unwrap_or_default()
        ));
        return outcome;
    }

    let servers = match object.get("mcpServers") {
        Some(Value::Object(servers)) => servers,
        _ => {
            outcome.disabled_reason =
                Some("missing or non-object required `mcpServers`".to_string());
            return outcome;
        }
    };

    for (name, server) in servers {
        if name.is_empty() {
            outcome.invalid.push(InvalidMcpServer {
                identity: format!("{plugin_name}/"),
                error: "server name must not be empty".to_string(),
            });
            continue;
        }
        match translate_server(name, server, root, data_dir) {
            Ok(config) => outcome.servers.push((name.clone(), config)),
            Err(ServerRejection::Invalid(error)) => outcome.invalid.push(InvalidMcpServer {
                identity: format!("{plugin_name}/{name}"),
                error,
            }),
            Err(ServerRejection::UnsupportedTransport(transport)) => outcome
                .skipped_unsupported
                .push(format!("{plugin_name}/{name} ({transport})")),
        }
    }

    // The spec requires the PLUGIN_DATA directory to exist before any plugin
    // subprocess starts; create it only when a stdio server will use it.
    let has_stdio = outcome
        .servers
        .iter()
        .any(|(_, config)| matches!(config.transport, McpTransport::Stdio { .. }));
    if has_stdio {
        if let Err(error) = std::fs::create_dir_all(data_dir) {
            let reason = format!("cannot create PLUGIN_DATA directory: {error}");
            let mut remaining = Vec::new();
            for (name, config) in outcome.servers.drain(..) {
                if matches!(config.transport, McpTransport::Stdio { .. }) {
                    outcome.invalid.push(InvalidMcpServer {
                        identity: format!("{plugin_name}/{name}"),
                        error: reason.clone(),
                    });
                } else {
                    remaining.push((name, config));
                }
            }
            outcome.servers = remaining;
        }
    }

    outcome
}

fn translate_server(
    name: &str,
    value: &Value,
    root: &Path,
    data_dir: &Path,
) -> Result<McpServerConfig, ServerRejection> {
    let invalid = |error: String| Err(ServerRejection::Invalid(error));
    let Some(object) = value.as_object() else {
        return invalid(format!("server `{name}` must be a JSON object"));
    };

    let transport = match object.get("type").and_then(Value::as_str) {
        Some("stdio") => translate_stdio(name, object, root, data_dir)?,
        Some("streamable-http") => translate_remote(name, object, "streamable-http")?,
        Some("sse") => return Err(ServerRejection::UnsupportedTransport("sse")),
        Some(other) => return invalid(format!("server `{name}` has unknown transport `{other}`")),
        None => return invalid(format!("server `{name}` is missing required `type`")),
    };

    Ok(McpServerConfig {
        enabled: true,
        tools: McpToolFilter::default(),
        transport,
    })
}

fn translate_stdio(
    name: &str,
    object: &serde_json::Map<String, Value>,
    root: &Path,
    data_dir: &Path,
) -> Result<McpTransport, ServerRejection> {
    let invalid = |error: String| Err(ServerRejection::Invalid(error));
    for key in object.keys() {
        if !matches!(key.as_str(), "type" | "command" | "args" | "env" | "cwd") {
            return invalid(format!("server `{name}` has unknown field `{key}`"));
        }
    }

    let root_display = root.to_string_lossy().to_string();
    let data_display = data_dir.to_string_lossy().to_string();

    // `command` is one executable token; placeholders never apply to it.
    let command = match object.get("command").and_then(Value::as_str) {
        Some(command) if !command.trim().is_empty() => command,
        _ => return invalid(format!("server `{name}` requires a non-empty `command`")),
    };
    let command = if command.contains('/') || command.contains('\\') {
        let Some(tail) = command.strip_prefix("./") else {
            return invalid(format!(
                "server `{name}` command must be a bare executable name or start with ./"
            ));
        };
        contain::resolve_in_root(root, tail)
            .map_err(|error| ServerRejection::Invalid(format!("server `{name}`: {error}")))?
            .to_string_lossy()
            .to_string()
    } else {
        command.to_string()
    };

    let mut args = Vec::new();
    if let Some(value) = object.get("args") {
        let Some(items) = value.as_array() else {
            return invalid(format!(
                "server `{name}` `args` must be an array of strings"
            ));
        };
        for item in items {
            let Some(item) = item.as_str() else {
                return invalid(format!("server `{name}` `args` entries must be strings"));
            };
            args.push(expand_placeholders(item, &root_display, &data_display));
        }
    }

    let mut env = BTreeMap::new();
    if let Some(value) = object.get("env") {
        let Some(entries) = value.as_object() else {
            return invalid(format!(
                "server `{name}` `env` must be an object of strings"
            ));
        };
        for (key, entry) in entries {
            if key == "PLUGIN_ROOT" || key == "PLUGIN_DATA" {
                return invalid(format!(
                    "server `{name}` `env` must not set reserved variable `{key}`"
                ));
            }
            // Windows environment names are case-insensitive, so case variants
            // of the reserved names are dropped before the client sets them.
            #[cfg(windows)]
            if key.eq_ignore_ascii_case("PLUGIN_ROOT") || key.eq_ignore_ascii_case("PLUGIN_DATA") {
                continue;
            }
            let Some(entry) = entry.as_str() else {
                return invalid(format!("server `{name}` `env` values must be strings"));
            };
            env.insert(
                key.clone(),
                expand_placeholders(entry, &root_display, &data_display),
            );
        }
    }
    // Client-provided reserved variables are set last, per spec §9.1.
    env.insert("PLUGIN_ROOT".to_string(), root_display);
    env.insert("PLUGIN_DATA".to_string(), data_display);

    let cwd = match object.get("cwd").and_then(Value::as_str) {
        None => root.to_path_buf(),
        Some(cwd) => resolve_cwd(name, cwd, root, data_dir)?,
    };

    Ok(McpTransport::Stdio {
        command,
        args,
        cwd: Some(cwd),
        env,
        env_from_env: BTreeMap::new(),
    })
}

fn resolve_cwd(
    name: &str,
    cwd: &str,
    root: &Path,
    data_dir: &Path,
) -> Result<std::path::PathBuf, ServerRejection> {
    let invalid = |error: String| Err(ServerRejection::Invalid(error));
    let resolve = |base: &Path, tail: &str| {
        contain::resolve_in_root(base, tail)
            .map_err(|error| ServerRejection::Invalid(format!("server `{name}` cwd: {error}")))
    };
    if let Some(tail) = cwd.strip_prefix("./") {
        return resolve(root, tail);
    }
    if cwd == PLUGIN_ROOT_PLACEHOLDER {
        return Ok(root.to_path_buf());
    }
    if let Some(tail) = cwd.strip_prefix(&format!("{PLUGIN_ROOT_PLACEHOLDER}/")) {
        return resolve(root, tail);
    }
    if cwd == PLUGIN_DATA_PLACEHOLDER {
        return Ok(data_dir.to_path_buf());
    }
    if let Some(tail) = cwd.strip_prefix(&format!("{PLUGIN_DATA_PLACEHOLDER}/")) {
        return resolve(data_dir, tail);
    }
    invalid(format!(
        "server `{name}` cwd must be plugin-relative, ${{PLUGIN_ROOT}}-rooted, or ${{PLUGIN_DATA}}-rooted"
    ))
}

fn translate_remote(
    name: &str,
    object: &serde_json::Map<String, Value>,
    transport: &str,
) -> Result<McpTransport, ServerRejection> {
    let invalid = |error: String| Err(ServerRejection::Invalid(error));
    for key in object.keys() {
        if !matches!(key.as_str(), "type" | "url" | "headers") {
            return invalid(format!("server `{name}` has unknown field `{key}`"));
        }
    }

    let url = match object.get("url").and_then(Value::as_str) {
        Some(url) if !url.is_empty() => url,
        _ => return invalid(format!("server `{name}` requires a non-empty `url`")),
    };
    let parsed = url::Url::parse(url).map_err(|error| {
        ServerRejection::Invalid(format!("server `{name}` has an invalid URL: {error}"))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return invalid(format!("server `{name}` URL must use http or https"));
    }
    if parsed.host().is_none() {
        return invalid(format!("server `{name}` URL must have a host"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return invalid(format!(
            "server `{name}` URL must not contain user information"
        ));
    }
    if parsed.fragment().is_some() {
        return invalid(format!("server `{name}` URL must not contain a fragment"));
    }
    crate::tools::mcp::validate_remote_url(url)
        .map_err(|error| ServerRejection::Invalid(format!("server `{name}`: {error}")))?;

    // Placeholders and environment expansion never apply to URLs or headers.
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    if let Some(value) = object.get("headers") {
        let Some(entries) = value.as_object() else {
            return invalid(format!(
                "server `{name}` `headers` must be an object of strings"
            ));
        };
        for (header, entry) in entries {
            let Some(entry) = entry.as_str() else {
                return invalid(format!("server `{name}` header values must be strings"));
            };
            HeaderName::try_from(header).map_err(|error| {
                ServerRejection::Invalid(format!(
                    "server `{name}` has an invalid header name: {error}"
                ))
            })?;
            HeaderValue::try_from(entry).map_err(|error| {
                ServerRejection::Invalid(format!(
                    "server `{name}` has an invalid header value: {error}"
                ))
            })?;
            let lower = header.to_ascii_lowercase();
            if headers
                .keys()
                .any(|existing| existing.to_ascii_lowercase() == lower)
            {
                return invalid(format!(
                    "server `{name}` repeats header `{header}` under different casing"
                ));
            }
            headers.insert(header.clone(), entry.to_string());
        }
    }

    debug_assert_eq!(transport, "streamable-http");
    Ok(McpTransport::StreamableHttp {
        url: url.to_string(),
        headers,
        headers_from_env: BTreeMap::new(),
    })
}

/// Single-pass, non-recursive expansion of `${PLUGIN_ROOT}` and
/// `${PLUGIN_DATA}`. Replacement text is never rescanned, and unrecognized
/// placeholder-like text stays literal (Agent Plugins spec §9.2).
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

fn schema_version(schema: &str) -> Option<&str> {
    let after = schema.strip_prefix("https://agent-plugins.org/schemas/")?;
    let version = after.split('/').next()?;
    (!version.is_empty()).then_some(version)
}
