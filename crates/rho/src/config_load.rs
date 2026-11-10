//! Loading `config.toml` into [`Config`] and saving changes back.
//!
//! Scalar settings go through the registry ([`AGENT_SETTINGS`]); the
//! structured tables in [`FILE_ONLY`] keep their serde types and are edited
//! by hand. Cross-field rules (alias targets, provider profiles, the
//! compaction pair) run once every key has loaded.

use std::collections::BTreeMap;

use serde::Deserialize;
use toml_edit::{Array, Item, Value};

use {
    crate::model_aliases::ModelAliases,
    rho_providers::model::favorites::{favorite_model_values, normalized_favorite_models},
    rho_providers::reasoning::ReasoningLevel,
};

use super::{
    document::{decode_table, encode_table, ConfigDocument},
    inferred_provider_auth,
    provider_config::{PartialProviderConfigs, PersistedProviderConfigs},
    registry::AGENT_SETTINGS,
    setting::{load_settings, save_settings, unknown_keys, AnySetting, ConfigError},
    Config, ConfigWarning, InternalAgentModelConfig, InternalAgentTarget, ModelKind,
    CLAUDE_CLI_RUNTIME_KEY,
};

/// A hand-edited structured table (or list) and how it saves.
struct FileOnly {
    key: &'static str,
    label: &'static str,
    /// The docs page, from the docs site root, that describes the table.
    docs: &'static str,
    /// The item to write; `None` removes the key.
    encode: fn(&Config) -> anyhow::Result<Option<Item>>,
}

/// Keys that keep their own serde types instead of registry rows. Loading
/// each is spelled out in [`resolve`].
const FILE_ONLY: [FileOnly; 7] = [
    FileOnly {
        key: "model.aliases",
        label: "Model aliases",
        docs: "/configuration#model-aliases",
        encode: |config| non_empty(&config.model_aliases, ModelAliases::is_empty),
    },
    FileOnly {
        key: "model.favorite_models",
        label: "Favorite models",
        docs: "/configuration#common-settings",
        encode: |config| {
            Ok((!config.favorite_models.is_empty()).then(|| {
                Item::Value(Value::Array(Array::from_iter(
                    config.favorite_models.iter().map(String::as_str),
                )))
            }))
        },
    },
    FileOnly {
        key: "internal_agents",
        label: "Internal agent models",
        docs: "/configuration#internal-agent-models",
        encode: |config| {
            let entries = config
                .internal_agents
                .iter()
                .map(|(id, selection)| {
                    (
                        id.as_str(),
                        PersistedInternalAgent::new(selection, &config.model_aliases),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            non_empty(&entries, BTreeMap::is_empty)
        },
    },
    FileOnly {
        key: "providers",
        label: "Provider profiles",
        docs: "/providers/openai-compatible",
        encode: |config| {
            non_empty(
                &PersistedProviderConfigs::from(&config.providers),
                PersistedProviderConfigs::is_empty,
            )
        },
    },
    FileOnly {
        key: "mcp",
        label: "MCP servers",
        docs: "/integrations/mcp#configuration-source-and-precedence",
        encode: |config| non_empty(&config.mcp, crate::tools::mcp::config::McpConfig::is_empty),
    },
    FileOnly {
        key: "openapi",
        label: "OpenAPI sources",
        docs: "/integrations/openapi",
        encode: |config| {
            non_empty(
                &config.openapi,
                crate::tools::openapi::config::OpenApiConfig::is_empty,
            )
        },
    },
    FileOnly {
        key: "prompt_templates",
        label: "Prompt templates",
        docs: "/configuration#prompt-templates",
        encode: |config| non_empty(&config.prompt_templates, BTreeMap::is_empty),
    },
];

fn non_empty<T: serde::Serialize>(
    value: &T,
    is_empty: fn(&T) -> bool,
) -> anyhow::Result<Option<Item>> {
    if is_empty(value) {
        return Ok(None);
    }
    encode_table(value).map(Some)
}

/// Whether `key` lies inside a file-only table. Registry rows there (the
/// internal-agent model leaves) describe the setting for clients; the
/// table's own loader reads and writes it.
fn in_file_only_table(key: &str) -> bool {
    FILE_ONLY.iter().any(|table| {
        key.strip_prefix(table.key)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    })
}

fn registry_rows() -> Vec<&'static dyn AnySetting<Config>> {
    AGENT_SETTINGS
        .iter()
        .copied()
        .filter(|setting| !in_file_only_table(setting.key()))
        .collect()
}

/// The tables `config.toml` keeps that only a hand edit changes.
pub(super) fn file_only_tables() -> Vec<rho_protocol::FileOnlyTable> {
    FILE_ONLY
        .iter()
        .map(|table| rho_protocol::FileOnlyTable {
            key: table.key.into(),
            label: table.label.into(),
            docs: table.docs.into(),
        })
        .collect()
}

pub(super) fn emit_warnings(path_display: &str, warnings: &[ConfigWarning]) {
    for warning in warnings {
        eprintln!("warning: {path_display}: {}", warning.message());
    }
}

/// One error listing every problem, one per line.
pub(super) fn errors_error(errors: &[ConfigError]) -> anyhow::Error {
    anyhow::anyhow!(errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n"))
}

/// The config `doc` describes, or every problem that keeps it from loading.
pub(crate) fn resolve(
    doc: &ConfigDocument,
) -> Result<(Config, Vec<ConfigWarning>), Vec<ConfigError>> {
    let mut config = Config::default();
    let mut warnings = Vec::new();
    let file_only_keys = FILE_ONLY.map(|table| table.key);
    let mut errors = unknown_keys(doc, AGENT_SETTINGS, &file_only_keys);
    load_settings(&registry_rows(), doc, &mut config, &mut errors);

    let mut internal_agents = None;
    for key in file_only_keys {
        let Some(item) = doc.get(key) else {
            continue;
        };
        let loaded = match key {
            "model.aliases" => decode_table(item).map(|aliases| config.model_aliases = aliases),
            "model.favorite_models" => favorite_models(item).map(|models| {
                config.favorite_models =
                    favorite_model_values(&normalized_favorite_models(&models));
            }),
            "internal_agents" => decode_table::<BTreeMap<String, PartialInternalAgent>>(item)
                .map(|entries| internal_agents = Some(entries)),
            "providers" => decode_table::<PartialProviderConfigs>(item).and_then(|providers| {
                config
                    .providers
                    .apply(providers, &mut warnings)
                    .map_err(|error| format!("{error:#}"))
            }),
            "mcp" => decode_table(item).map(|mcp| config.mcp = mcp),
            "openapi" => decode_table(item).map(|openapi| config.openapi = openapi),
            "prompt_templates" => decode_table(item).and_then(|templates| {
                crate::prompt_templates::validate(&templates)
                    .map_err(|error| format!("{error:#}"))?;
                config.prompt_templates = templates;
                Ok(())
            }),
            other => unreachable!("FILE_ONLY lists {other} without a loader"),
        };
        if let Err(message) = loaded {
            errors.push(ConfigError::at(key, message));
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let cross_field = |result: anyhow::Result<()>, errors: &mut Vec<ConfigError>| {
        if let Err(error) = result {
            errors.push(ConfigError {
                key: None,
                message: format!("{error:#}"),
            });
        }
    };
    cross_field(config.validate_model_aliases(), &mut errors);
    cross_field(config.resolve_model_alias(), &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    config.internal_agents = internal_agents
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, entry)| {
            if catalog_entry_cannot_override(&id, &entry, &config.model_aliases) {
                warnings.push(ConfigWarning::Normalized {
                    key: "internal_agents",
                    from: format!("for agent '{id}'"),
                    to: "no override; a catalog agent's entry must name a Rho provider and model"
                        .into(),
                });
                return None;
            }
            let selection = internal_agent_selection(&id, entry, &config, &mut warnings);
            Some((id, selection))
        })
        .collect();
    cross_field(config.resolve_internal_agent_model_aliases(), &mut errors);
    cross_field(config.normalize_provider_profiles(), &mut errors);
    if config.compact_target_percent >= config.compact_threshold_percent {
        errors.push(ConfigError::at(
            "compaction.compact_target_percent",
            format!(
                "must be below compaction.compact_threshold_percent ({}); got {}",
                config.compact_threshold_percent, config.compact_target_percent
            ),
        ));
    }
    if errors.is_empty() {
        Ok((config, warnings))
    } else {
        Err(errors)
    }
}

fn favorite_models(item: &Item) -> Result<Vec<String>, String> {
    let not_a_list = || "must be a list of model references".to_owned();
    item.as_array()
        .ok_or_else(not_a_list)?
        .iter()
        .map(|value| value.as_str().map(str::to_owned).ok_or_else(not_a_list))
        .collect()
}

/// Writes to `doc` every value that differs between `before` and `after`,
/// leaving every other line of the file as written.
pub(super) fn save_diff(
    before: &Config,
    after: &Config,
    doc: &mut ConfigDocument,
) -> anyhow::Result<()> {
    save_settings(&registry_rows(), before, after, doc);
    for table in &FILE_ONLY {
        let new = (table.encode)(after)?;
        let old = (table.encode)(before)?;
        if rendered(old.as_ref()) != rendered(new.as_ref()) {
            doc.set_item(table.key, new);
        }
    }
    Ok(())
}

/// `item` as a file holding it would read. An item's own `Display` leaves
/// out the sub-tables of an implicit table, which is most of what
/// `internal_agents` holds, so two different tables could print alike.
fn rendered(item: Option<&Item>) -> Option<String> {
    item.map(|item| {
        let mut doc = toml_edit::DocumentMut::new();
        doc.insert("table", item.clone());
        doc.to_string()
    })
}

/// On-disk form of one `[internal_agents.<id>]` entry.
///
/// `runtime` is omitted for Rho selections. A `claude-cli` entry carries
/// only the pass-through model: it has no Rho provider and no Rho auth.
#[derive(serde::Serialize)]
struct PersistedInternalAgent<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<ModelKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allow_threshold_percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningLevel>,
}

impl<'a> PersistedInternalAgent<'a> {
    fn new(selection: &'a InternalAgentModelConfig, aliases: &'a ModelAliases) -> Self {
        match &selection.target {
            InternalAgentTarget::Rho(rho) => Self {
                runtime: None,
                provider: Some(rho.provider.as_str()),
                model: Some(match selection.current_alias(aliases) {
                    Some(alias) => format!("@{alias}"),
                    None => rho.model.clone(),
                }),
                auth: Some(rho.auth.as_str()),
                kind: rho.kind,
                allow_threshold_percent: rho.allow_threshold_percent,
                reasoning: selection.reasoning,
            },
            InternalAgentTarget::ClaudeCli { model } => Self {
                runtime: Some(CLAUDE_CLI_RUNTIME_KEY),
                provider: None,
                model: model.clone(),
                auth: None,
                kind: None,
                allow_threshold_percent: None,
                reasoning: selection.reasoning,
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialInternalAgent {
    /// `rho` (default) or `claude-cli`.
    runtime: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    auth: Option<String>,
    /// How a decision-model entry asks its model; see [`ModelKind`].
    kind: Option<ModelKind>,
    /// The P(allow) percent at which a decision-model permission screen
    /// allows. Range-checked where the screen is built, so a bad value names
    /// its limit instead of failing the whole config load.
    allow_threshold_percent: Option<u8>,
    reasoning: Option<ReasoningLevel>,
}

/// Whether an entry for a catalog agent (an id Rho does not reserve, such
/// as a plugin agent's) fails to name an explicit Rho provider and model.
/// A `@alias` model whose target names a provider names both.
/// Such an entry replaces the model the agent's file names, so the
/// conversation-model fallback internal agents use would swap models nobody
/// asked to swap; config load drops it instead.
fn catalog_entry_cannot_override(
    id: &str,
    group: &PartialInternalAgent,
    aliases: &ModelAliases,
) -> bool {
    let catalog =
        crate::agent::AgentId::new(id).is_ok_and(|id| !crate::agent::is_reserved_agent_id(&id));
    let rho = matches!(
        group.runtime.as_deref(),
        None | Some(crate::config::RHO_RUNTIME_KEY)
    );
    // An undefined alias counts as named, so alias resolution fails the load
    // loudly instead of this check dropping the entry.
    let alias_names_provider = |model: &str| {
        aliases
            .resolve(model)
            .map_or(true, |resolved| resolved.provider.is_some())
    };
    let names_model = match (&group.provider, group.model.as_deref()) {
        (Some(_), Some(_)) => true,
        (None, Some(model)) => model.starts_with('@') && alias_names_provider(model),
        (_, None) => false,
    };
    catalog && !(rho && names_model)
}

/// Builds one internal-agent selection, filling Rho defaults from the
/// conversation model.
///
/// An unusable `runtime` value falls back to the whole conversation selection
/// rather than failing the config load, and Rho-only keys on a `claude-cli`
/// entry are reported instead of silently dropped.
fn internal_agent_selection(
    id: &str,
    group: PartialInternalAgent,
    cfg: &Config,
    warnings: &mut Vec<ConfigWarning>,
) -> InternalAgentModelConfig {
    let runtime = internal_agent_runtime_key(id, group.runtime.as_deref(), warnings);
    let mut selection = match runtime {
        InternalAgentRuntimeKey::Rho => {
            let provider = group.provider.unwrap_or_else(|| cfg.provider.clone());
            let auth = group
                .auth
                .unwrap_or_else(|| inferred_provider_auth(&provider, &cfg.provider, &cfg.auth));
            let model = group.model.unwrap_or_else(|| cfg.model.clone());
            let mut selection = InternalAgentModelConfig::new(provider, model, auth);
            if let InternalAgentTarget::Rho(rho) = &mut selection.target {
                rho.kind = group.kind;
                rho.allow_threshold_percent = group.allow_threshold_percent;
            }
            selection
        }
        // A rejected runtime takes the whole entry with it. Provider, model,
        // and auth were written to work together under the runtime we are not
        // using, so keeping any one of them risks a pairing no runtime asked
        // for - a conversation model routed through the entry's credentials.
        InternalAgentRuntimeKey::Rejected => {
            InternalAgentModelConfig::new(cfg.provider.clone(), cfg.model.clone(), cfg.auth.clone())
        }
        InternalAgentRuntimeKey::ClaudeCli => {
            for (key, value) in [
                ("internal_agents.provider", group.provider),
                ("internal_agents.auth", group.auth),
            ] {
                if let Some(value) = value {
                    warnings.push(ConfigWarning::Normalized {
                        key,
                        from: format!("\"{value}\""),
                        to: format!(
                            "no value; [internal_agents.{id}] runtime claude-cli has no Rho provider or auth"
                        ),
                    });
                }
            }
            InternalAgentModelConfig::claude_cli(group.model)
        }
    };
    // `reasoning` is Rho vocabulary on both runtimes, so it survives either way.
    selection.reasoning = group.reasoning;
    selection
}

/// Reads the `runtime` key of one `[internal_agents.<id>]` entry, reporting
/// every value it cannot honor.
fn internal_agent_runtime_key(
    id: &str,
    runtime: Option<&str>,
    warnings: &mut Vec<ConfigWarning>,
) -> InternalAgentRuntimeKey {
    let (from, to) = match runtime {
        None | Some(crate::config::RHO_RUNTIME_KEY) => return InternalAgentRuntimeKey::Rho,
        Some(crate::config::CLAUDE_CLI_RUNTIME_KEY)
            if crate::agent::internal_agent_accepts_claude_runtime(id) =>
        {
            return InternalAgentRuntimeKey::ClaudeCli
        }
        Some(crate::config::CLAUDE_CLI_RUNTIME_KEY) => (
            crate::config::CLAUDE_CLI_RUNTIME_KEY,
            format!(
                "\"{}\" with the conversation selection; agent '{id}' cannot delegate",
                crate::config::RHO_RUNTIME_KEY
            ),
        ),
        Some(crate::config::CURSOR_RUNTIME_KEY) => (
            crate::config::CURSOR_RUNTIME_KEY,
            format!(
                "\"{}\" with the conversation selection; [internal_agents] runtime stays rho or claude-cli, not cursor",
                crate::config::RHO_RUNTIME_KEY
            ),
        ),
        Some(other) => (
            other,
            format!(
                "\"{}\" with the conversation selection",
                crate::config::RHO_RUNTIME_KEY
            ),
        ),
    };
    warnings.push(ConfigWarning::Normalized {
        key: "internal_agents.runtime",
        from: format!("\"{from}\""),
        to,
    });
    InternalAgentRuntimeKey::Rejected
}

/// Parsed `runtime` key for an internal-agent entry.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InternalAgentRuntimeKey {
    Rho,
    ClaudeCli,
    /// A value we cannot honor. Runs on Rho with the conversation provider,
    /// model, and auth, ignoring every override on the entry.
    Rejected,
}

#[cfg(test)]
#[path = "config_load_tests.rs"]
mod tests;
