use std::collections::BTreeMap;

use serde::Deserialize;

use {
    crate::keybindings::Keybindings,
    crate::model_aliases::ModelAliases,
    crate::permission::PermissionMode,
    rho_providers::credentials::CredentialStoreBackend,
    rho_providers::model::favorites::{favorite_model_values, normalized_favorite_models},
    rho_providers::reasoning::ReasoningLevel,
};

use super::{
    format::InternalAgentModelConfig, inferred_provider_auth,
    provider_config::PartialProviderConfigs, Config, LegacyWebSearchCredentials, SearchProvider,
};

/// Non-fatal issue found while loading config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConfigWarning {
    UnknownKey {
        path: String,
    },
    Clamped {
        key: &'static str,
        from: String,
        to: String,
    },
    Normalized {
        key: &'static str,
        from: String,
        to: String,
    },
}

impl ConfigWarning {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::UnknownKey { path } => {
                format!("unknown config key `{path}` (ignored)")
            }
            Self::Clamped { key, from, to } => {
                format!("config `{key}` value {from} is out of range; using {to}")
            }
            Self::Normalized { key, from, to } => {
                format!("config `{key}` value {from} is unsupported; using {to}")
            }
        }
    }
}

pub(super) fn emit_warnings(path_display: &str, warnings: &[ConfigWarning]) {
    for warning in warnings {
        eprintln!("warning: {path_display}: {}", warning.message());
    }
}

pub(super) fn parse_settings(text: &str) -> anyhow::Result<(Config, Vec<ConfigWarning>)> {
    let raw: toml::Value = toml::from_str(text)?;
    let mut warnings = collect_unknown_keys(&raw);
    let file = PartialConfig::deserialize(raw)?.normalize_legacy()?;
    let mut cfg = Config::default();

    if let Some(v) = file.prompt_templates {
        crate::prompt_templates::validate(&v)?;
        cfg.prompt_templates = v;
    }
    if let Some(v) = file.provider {
        cfg.provider = v;
    }
    if let Some(v) = file.auth {
        cfg.auth = v;
    }
    if let Some(v) = file.reasoning {
        cfg.reasoning = v;
    }
    if let Some(v) = file.fast_mode {
        cfg.fast_mode = v;
    }
    if let Some(v) = file.favorite_models {
        cfg.favorite_models = favorite_model_values(&normalized_favorite_models(&v));
    }
    match file.model {
        Some(ModelSetting::Name(model)) => cfg.model = model,
        Some(ModelSetting::Group(group)) => {
            if let Some(provider) = group.provider {
                cfg.provider = provider;
            }
            if let Some(model) = group.model {
                cfg.model = model;
            }
            if let Some(auth) = group.auth {
                cfg.auth = auth;
            }
            if let Some(reasoning) = group.reasoning {
                cfg.reasoning = reasoning;
            }
            if let Some(fast_mode) = group.fast_mode {
                cfg.fast_mode = fast_mode;
            }
            if let Some(models) = group.favorite_models {
                cfg.favorite_models = favorite_model_values(&normalized_favorite_models(&models));
            }
            if let Some(aliases) = group.aliases {
                cfg.model_aliases = aliases;
            }
        }
        None => {}
    }
    cfg.validate_model_aliases()?;
    cfg.resolve_model_alias()?;
    if let Some(group) = file.display {
        if let Some(value) = group.show_reasoning_output {
            cfg.show_reasoning_output = value;
        }
        if let Some(value) = group.max_tool_output_lines {
            let clamped = value.max(1);
            if clamped != value {
                warnings.push(ConfigWarning::Clamped {
                    key: "display.max_tool_output_lines",
                    from: value.to_string(),
                    to: clamped.to_string(),
                });
            }
            cfg.max_tool_output_lines = clamped;
        }
    }
    if let Some(group) = file.output {
        if let Some(value) = group.max_output_bytes {
            cfg.max_output_bytes = value;
        }
    }
    if let Some(group) = file.compaction {
        if let Some(value) = group.auto_compact {
            cfg.auto_compact = value;
        }
        if let Some(value) = group.compact_threshold_percent {
            cfg.set_compact_threshold_percent(value);
            note_if_changed(
                &mut warnings,
                "compaction.compact_threshold_percent",
                value,
                cfg.compact_threshold_percent,
            );
        }
        if let Some(value) = group.compact_target_percent {
            cfg.set_compact_target_percent(value);
            note_if_changed(
                &mut warnings,
                "compaction.compact_target_percent",
                value,
                cfg.compact_target_percent,
            );
        }
    }
    cfg.internal_agents = file
        .internal_agents
        .unwrap_or_default()
        .into_iter()
        .map(|(id, group)| {
            let provider = group.provider.unwrap_or_else(|| cfg.provider.clone());
            let auth = group
                .auth
                .unwrap_or_else(|| inferred_provider_auth(&provider, &cfg.provider, &cfg.auth));
            (
                id,
                InternalAgentModelConfig {
                    provider,
                    model: group.model.unwrap_or_else(|| cfg.model.clone()),
                    auth,
                    model_alias: None,
                },
            )
        })
        .collect();
    if let Some(group) = file.title {
        let provider = group.provider.unwrap_or_else(|| cfg.provider.clone());
        let auth = group
            .auth
            .unwrap_or_else(|| inferred_provider_auth(&provider, &cfg.provider, &cfg.auth));
        cfg.internal_agents
            .entry("session-title".into())
            .or_insert(InternalAgentModelConfig {
                provider,
                model: group.model.unwrap_or_else(|| cfg.model.clone()),
                auth,
                model_alias: None,
            });
    }
    cfg.resolve_internal_agent_model_aliases()?;
    cfg.normalize_provider_profiles()?;
    if let Some(group) = file.web_search {
        if let Some(hosted) = group.hosted {
            cfg.web_search_hosted = hosted;
        }
        if let Some(provider) = group.provider {
            let (parsed, normalized) = SearchProvider::parse_config_value(&provider);
            if normalized {
                warnings.push(ConfigWarning::Normalized {
                    key: "web_search.provider",
                    from: format!("\"{provider}\""),
                    to: format!("\"{}\"", parsed.as_str()),
                });
            }
            cfg.web_search_provider = parsed;
        }
        cfg.legacy_web_search_credentials = LegacyWebSearchCredentials {
            openai: group.openai_api_key.and_then(non_empty_secret),
            exa: group.exa_api_key.and_then(non_empty_secret),
            brave: group.brave_api_key.and_then(non_empty_secret),
        };
    }
    if let Some(providers) = file.providers {
        cfg.providers.apply(providers)?;
    }
    if let Some(group) = file.behavior {
        if let Some(value) = group.check_for_updates {
            cfg.check_for_updates = value;
        }
        if let Some(value) = group.enable_subagents {
            cfg.enable_subagents = value;
        }
        if let Some(value) = group.experimental_workspace_rewind {
            cfg.experimental_workspace_rewind = value;
        }
        if let Some(value) = group.permission_mode {
            cfg.permission_mode = value;
        }
        if let Some(value) = group.credential_store.as_deref() {
            cfg.credential_store = Some(
                CredentialStoreBackend::parse(value)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?,
            );
        }
        if let Some(value) = group.rtk {
            cfg.rtk = value;
        }
        if let Some(value) = group.inline_shell.filter(|value| !value.trim().is_empty()) {
            cfg.inline_shell = value;
        }
    }
    if let Some(keybindings) = file.keybindings {
        cfg.keybindings = keybindings;
    }
    Ok((cfg, warnings))
}

fn note_if_changed(
    warnings: &mut Vec<ConfigWarning>,
    key: &'static str,
    requested: u8,
    actual: u8,
) {
    if requested != actual {
        warnings.push(ConfigWarning::Clamped {
            key,
            from: requested.to_string(),
            to: actual.to_string(),
        });
    }
}

fn non_empty_secret(secret: String) -> Option<String> {
    let secret = secret.trim().to_string();
    (!secret.is_empty()).then_some(secret)
}

const TOP_LEVEL_KEYS: &[&str] = &[
    "provider",
    "model",
    "max_output_bytes",
    "max_tool_output_lines",
    "auth",
    "reasoning",
    "fast_mode",
    "reasoning_effort",
    "show_reasoning_output",
    "auto_compact",
    "compact_threshold_percent",
    "compact_target_percent",
    "title_provider",
    "title_model",
    "title_auth",
    "favorite_models",
    "web_search_provider",
    "check_for_updates",
    "enable_subagents",
    "permission_mode",
    "web_search_openai_api_key",
    "web_search_exa_api_key",
    "web_search_brave_api_key",
    "rtk",
    "inline_shell",
    "display",
    "output",
    "compaction",
    "title",
    "internal_agents",
    "web_search",
    "behavior",
    "keybindings",
    "prompt_templates",
    "providers",
];

const MODEL_KEYS: &[&str] = &[
    "provider",
    "model",
    "auth",
    "reasoning",
    "fast_mode",
    "favorite_models",
    "aliases",
];
const DISPLAY_KEYS: &[&str] = &["show_reasoning_output", "max_tool_output_lines"];
const OUTPUT_KEYS: &[&str] = &["max_output_bytes"];
const COMPACTION_KEYS: &[&str] = &[
    "auto_compact",
    "compact_threshold_percent",
    "compact_target_percent",
];
const TITLE_KEYS: &[&str] = &["provider", "model", "auth"];
const INTERNAL_AGENT_KEYS: &[&str] = &["provider", "model", "auth"];
const WEB_SEARCH_KEYS: &[&str] = &[
    "hosted",
    "provider",
    "openai_api_key",
    "exa_api_key",
    "brave_api_key",
];
const BEHAVIOR_KEYS: &[&str] = &[
    "check_for_updates",
    "enable_subagents",
    "experimental_workspace_rewind",
    "permission_mode",
    "credential_store",
    "rtk",
    "inline_shell",
];
const KEYBINDING_KEYS: &[&str] = &[
    "reset_conversation",
    "open_editor",
    "jump_to_bottom",
    "toggle_tool_output",
    "insert_newline",
    "paste_image",
    "edit_pending_input",
    "manage_pending_input",
];
const PROVIDER_KEYS: &[&str] = &["ollama"];
const OLLAMA_KEYS: &[&str] = &["base_url"];

fn collect_unknown_keys(root: &toml::Value) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();
    let Some(table) = root.as_table() else {
        return warnings;
    };
    for (key, value) in table {
        if !TOP_LEVEL_KEYS.contains(&key.as_str()) {
            warnings.push(ConfigWarning::UnknownKey { path: key.clone() });
            continue;
        }
        match key.as_str() {
            "model" => collect_model_keys(value, &mut warnings),
            "display" => collect_table_keys(value, "display", DISPLAY_KEYS, &mut warnings),
            "output" => collect_table_keys(value, "output", OUTPUT_KEYS, &mut warnings),
            "compaction" => collect_table_keys(value, "compaction", COMPACTION_KEYS, &mut warnings),
            "title" => collect_table_keys(value, "title", TITLE_KEYS, &mut warnings),
            "web_search" => collect_table_keys(value, "web_search", WEB_SEARCH_KEYS, &mut warnings),
            "behavior" => collect_table_keys(value, "behavior", BEHAVIOR_KEYS, &mut warnings),
            "keybindings" => {
                collect_table_keys(value, "keybindings", KEYBINDING_KEYS, &mut warnings)
            }
            "internal_agents" => collect_internal_agent_keys(value, &mut warnings),
            "providers" => collect_providers_keys(value, &mut warnings),
            // Free-form user template names, or scalar/legacy top-level keys.
            _ => {}
        }
    }
    warnings
}

fn collect_model_keys(value: &toml::Value, warnings: &mut Vec<ConfigWarning>) {
    match value {
        toml::Value::Table(_) => collect_table_keys(value, "model", MODEL_KEYS, warnings),
        toml::Value::String(_) => {}
        _ => {}
    }
}

fn collect_internal_agent_keys(value: &toml::Value, warnings: &mut Vec<ConfigWarning>) {
    let Some(table) = value.as_table() else {
        return;
    };
    for (id, entry) in table {
        collect_table_keys(
            entry,
            &format!("internal_agents.{id}"),
            INTERNAL_AGENT_KEYS,
            warnings,
        );
    }
}

fn collect_providers_keys(value: &toml::Value, warnings: &mut Vec<ConfigWarning>) {
    let Some(table) = value.as_table() else {
        return;
    };
    for (key, entry) in table {
        if !PROVIDER_KEYS.contains(&key.as_str()) {
            warnings.push(ConfigWarning::UnknownKey {
                path: format!("providers.{key}"),
            });
            continue;
        }
        if key == "ollama" {
            collect_table_keys(entry, "providers.ollama", OLLAMA_KEYS, warnings);
        }
    }
}

fn collect_table_keys(
    value: &toml::Value,
    path: &str,
    known: &[&str],
    warnings: &mut Vec<ConfigWarning>,
) {
    let Some(table) = value.as_table() else {
        return;
    };
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            warnings.push(ConfigWarning::UnknownKey {
                path: format!("{path}.{key}"),
            });
        }
    }
}

#[derive(Deserialize)]
struct PartialConfig {
    provider: Option<String>,
    model: Option<ModelSetting>,
    max_output_bytes: Option<usize>,
    max_tool_output_lines: Option<usize>,
    auth: Option<String>,
    reasoning: Option<ReasoningLevel>,
    fast_mode: Option<bool>,
    reasoning_effort: Option<String>,
    show_reasoning_output: Option<bool>,
    auto_compact: Option<bool>,
    compact_threshold_percent: Option<u8>,
    compact_target_percent: Option<u8>,
    title_provider: Option<String>,
    title_model: Option<String>,
    title_auth: Option<String>,
    favorite_models: Option<Vec<String>>,
    web_search_provider: Option<String>,
    check_for_updates: Option<bool>,
    enable_subagents: Option<bool>,
    #[serde(default)]
    permission_mode: Option<PermissionMode>,
    web_search_openai_api_key: Option<String>,
    web_search_exa_api_key: Option<String>,
    web_search_brave_api_key: Option<String>,
    rtk: Option<bool>,
    inline_shell: Option<String>,
    display: Option<PartialDisplayConfig>,
    output: Option<PartialOutputConfig>,
    compaction: Option<PartialCompactionConfig>,
    title: Option<PartialTitleConfig>,
    internal_agents: Option<BTreeMap<String, PartialInternalAgentModelConfig>>,
    web_search: Option<PartialWebSearchConfig>,
    behavior: Option<PartialBehaviorConfig>,
    keybindings: Option<Keybindings>,
    prompt_templates: Option<crate::prompt_templates::PromptTemplates>,
    providers: Option<PartialProviderConfigs>,
}

impl PartialConfig {
    /// Fold every legacy top-level key into its modern group.
    ///
    /// Group values win when both a flat key and a group field are present,
    /// matching the previous parse order where groups were applied second.
    fn normalize_legacy(mut self) -> anyhow::Result<Self> {
        if self.reasoning.is_none() {
            if let Some(effort) = self.reasoning_effort.take() {
                self.reasoning = Some(effort.parse()?);
            }
        } else {
            self.reasoning_effort = None;
        }

        let show_reasoning_output = self.show_reasoning_output.take();
        let max_tool_output_lines = self.max_tool_output_lines.take();
        if show_reasoning_output.is_some()
            || max_tool_output_lines.is_some()
            || self.display.is_some()
        {
            let group = self.display.take().unwrap_or(PartialDisplayConfig {
                show_reasoning_output: None,
                max_tool_output_lines: None,
            });
            self.display = Some(PartialDisplayConfig {
                show_reasoning_output: group.show_reasoning_output.or(show_reasoning_output),
                max_tool_output_lines: group.max_tool_output_lines.or(max_tool_output_lines),
            });
        }

        let max_output_bytes = self.max_output_bytes.take();
        if max_output_bytes.is_some() || self.output.is_some() {
            let group = self.output.take().unwrap_or(PartialOutputConfig {
                max_output_bytes: None,
            });
            self.output = Some(PartialOutputConfig {
                max_output_bytes: group.max_output_bytes.or(max_output_bytes),
            });
        }

        let auto_compact = self.auto_compact.take();
        let compact_threshold_percent = self.compact_threshold_percent.take();
        let compact_target_percent = self.compact_target_percent.take();
        if auto_compact.is_some()
            || compact_threshold_percent.is_some()
            || compact_target_percent.is_some()
            || self.compaction.is_some()
        {
            let group = self.compaction.take().unwrap_or(PartialCompactionConfig {
                auto_compact: None,
                compact_threshold_percent: None,
                compact_target_percent: None,
            });
            self.compaction = Some(PartialCompactionConfig {
                auto_compact: group.auto_compact.or(auto_compact),
                compact_threshold_percent: group
                    .compact_threshold_percent
                    .or(compact_threshold_percent),
                compact_target_percent: group.compact_target_percent.or(compact_target_percent),
            });
        }

        let check_for_updates = self.check_for_updates.take();
        let enable_subagents = self.enable_subagents.take();
        let permission_mode = self.permission_mode.take();
        let rtk = self.rtk.take();
        let inline_shell = self.inline_shell.take();
        if check_for_updates.is_some()
            || enable_subagents.is_some()
            || permission_mode.is_some()
            || rtk.is_some()
            || inline_shell.is_some()
            || self.behavior.is_some()
        {
            let group = self.behavior.take().unwrap_or(PartialBehaviorConfig {
                check_for_updates: None,
                enable_subagents: None,
                experimental_workspace_rewind: None,
                permission_mode: None,
                credential_store: None,
                rtk: None,
                inline_shell: None,
            });
            self.behavior = Some(PartialBehaviorConfig {
                check_for_updates: group.check_for_updates.or(check_for_updates),
                enable_subagents: group.enable_subagents.or(enable_subagents),
                experimental_workspace_rewind: group.experimental_workspace_rewind,
                permission_mode: group.permission_mode.or(permission_mode),
                credential_store: group.credential_store,
                rtk: group.rtk.or(rtk),
                inline_shell: group.inline_shell.or(inline_shell),
            });
        }

        let web_search_provider = self.web_search_provider.take();
        let openai_api_key = self.web_search_openai_api_key.take();
        let exa_api_key = self.web_search_exa_api_key.take();
        let brave_api_key = self.web_search_brave_api_key.take();
        if web_search_provider.is_some()
            || openai_api_key.is_some()
            || exa_api_key.is_some()
            || brave_api_key.is_some()
            || self.web_search.is_some()
        {
            let group = self.web_search.take().unwrap_or(PartialWebSearchConfig {
                hosted: None,
                provider: None,
                openai_api_key: None,
                exa_api_key: None,
                brave_api_key: None,
            });
            self.web_search = Some(PartialWebSearchConfig {
                hosted: group.hosted,
                provider: group.provider.or(web_search_provider),
                openai_api_key: group.openai_api_key.or(openai_api_key),
                exa_api_key: group.exa_api_key.or(exa_api_key),
                brave_api_key: group.brave_api_key.or(brave_api_key),
            });
        }

        let title_provider = self.title_provider.take();
        let title_model = self.title_model.take();
        let title_auth = self.title_auth.take();
        match self.title.take() {
            Some(group)
                if group.provider.is_none() && group.model.is_none() && group.auth.is_none() =>
            {
                // Empty `[title]` is a no-op. Legacy top-level title_* keys still apply.
                if title_provider.is_some() || title_model.is_some() || title_auth.is_some() {
                    self.title = Some(PartialTitleConfig {
                        provider: title_provider,
                        model: title_model,
                        auth: title_auth,
                    });
                }
            }
            Some(group) => {
                self.title = Some(PartialTitleConfig {
                    provider: group.provider.or(title_provider),
                    model: group.model.or(title_model),
                    auth: group.auth.or(title_auth),
                });
            }
            None if title_provider.is_some() || title_model.is_some() || title_auth.is_some() => {
                self.title = Some(PartialTitleConfig {
                    provider: title_provider,
                    model: title_model,
                    auth: title_auth,
                });
            }
            None => {}
        }

        self.model = match self.model.take() {
            Some(ModelSetting::Group(mut group)) => {
                group.provider = group.provider.or(self.provider.take());
                group.auth = group.auth.or(self.auth.take());
                group.reasoning = group.reasoning.or(self.reasoning.take());
                group.fast_mode = group.fast_mode.or(self.fast_mode.take());
                group.favorite_models = group.favorite_models.or(self.favorite_models.take());
                Some(ModelSetting::Group(group))
            }
            other => other,
        };

        Ok(self)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ModelSetting {
    Name(String),
    Group(PartialModelConfig),
}

#[derive(Deserialize)]
struct PartialModelConfig {
    provider: Option<String>,
    model: Option<String>,
    auth: Option<String>,
    reasoning: Option<ReasoningLevel>,
    fast_mode: Option<bool>,
    favorite_models: Option<Vec<String>>,
    aliases: Option<ModelAliases>,
}

#[derive(Deserialize)]
struct PartialDisplayConfig {
    show_reasoning_output: Option<bool>,
    max_tool_output_lines: Option<usize>,
}

#[derive(Deserialize)]
struct PartialOutputConfig {
    max_output_bytes: Option<usize>,
}

#[derive(Deserialize)]
struct PartialCompactionConfig {
    auto_compact: Option<bool>,
    compact_threshold_percent: Option<u8>,
    compact_target_percent: Option<u8>,
}

#[derive(Deserialize)]
struct PartialInternalAgentModelConfig {
    provider: Option<String>,
    model: Option<String>,
    auth: Option<String>,
}

#[derive(Deserialize)]
struct PartialTitleConfig {
    provider: Option<String>,
    model: Option<String>,
    auth: Option<String>,
}

#[derive(Deserialize)]
struct PartialWebSearchConfig {
    hosted: Option<bool>,
    provider: Option<String>,
    openai_api_key: Option<String>,
    exa_api_key: Option<String>,
    brave_api_key: Option<String>,
}

#[derive(Deserialize)]
struct PartialBehaviorConfig {
    check_for_updates: Option<bool>,
    enable_subagents: Option<bool>,
    experimental_workspace_rewind: Option<bool>,
    #[serde(default)]
    permission_mode: Option<PermissionMode>,
    credential_store: Option<String>,
    rtk: Option<bool>,
    inline_shell: Option<String>,
}

#[cfg(test)]
#[path = "config_load_tests.rs"]
mod tests;
