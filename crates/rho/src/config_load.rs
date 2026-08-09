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
    provider_config::PartialProviderConfigs, Config, EditTool, LegacyWebSearchCredentials,
    SearchProvider,
};

/// Non-fatal issue found while loading config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConfigWarning {
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
    let file = toml::from_str::<PartialConfig>(text)?.normalize_legacy()?;
    let mut cfg = Config::default();
    let mut warnings = Vec::new();

    if let Some(v) = file.prompt_templates {
        crate::prompt_templates::validate(&v)?;
        cfg.prompt_templates = v;
    }
    if let Some(mcp) = file.mcp {
        cfg.mcp = mcp;
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
        if let Some(value) = group.zen_mode {
            cfg.zen_mode = value;
        }
        if let Some(value) = group.theme {
            let trimmed = value.trim();
            cfg.theme = if trimmed.is_empty() {
                "terminal".into()
            } else {
                trimmed.to_string()
            };
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
            let selection = internal_agent_selection(&id, group, &cfg, &mut warnings);
            (id, selection)
        })
        .collect();
    if let Some(group) = file.title {
        let provider = group.provider.unwrap_or_else(|| cfg.provider.clone());
        let auth = group
            .auth
            .unwrap_or_else(|| inferred_provider_auth(&provider, &cfg.provider, &cfg.auth));
        let model = group.model.unwrap_or_else(|| cfg.model.clone());
        cfg.internal_agents
            .entry("session-title".into())
            .or_insert_with(|| InternalAgentModelConfig::new(provider, model, auth));
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
        if let Some(value) = group.advisor_mode {
            cfg.advisor_mode = value;
        }
        if let Some(value) = group.experimental_workspace_rewind {
            cfg.experimental_workspace_rewind = value;
        }
        if let Some(value) = group.permission_mode {
            cfg.permission_mode = value;
        }
        if let Some(value) = group.edit_tool {
            cfg.edit_tool = value;
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

/// Raw file shape. Serde rejects unknown fields so a misspelled key fails loudly.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    zen_mode: Option<bool>,
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
    mcp: Option<crate::tools::mcp::config::McpConfig>,
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
        let zen_mode = self.zen_mode.take();
        let max_tool_output_lines = self.max_tool_output_lines.take();
        if show_reasoning_output.is_some()
            || zen_mode.is_some()
            || max_tool_output_lines.is_some()
            || self.display.is_some()
        {
            let group = self.display.take().unwrap_or(PartialDisplayConfig {
                show_reasoning_output: None,
                zen_mode: None,
                theme: None,
                max_tool_output_lines: None,
            });
            self.display = Some(PartialDisplayConfig {
                show_reasoning_output: group.show_reasoning_output.or(show_reasoning_output),
                zen_mode: group.zen_mode.or(zen_mode),
                theme: group.theme,
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
                advisor_mode: None,
                experimental_workspace_rewind: None,
                permission_mode: None,
                edit_tool: None,
                credential_store: None,
                rtk: None,
                inline_shell: None,
            });
            self.behavior = Some(PartialBehaviorConfig {
                check_for_updates: group.check_for_updates.or(check_for_updates),
                enable_subagents: group.enable_subagents.or(enable_subagents),
                advisor_mode: group.advisor_mode,
                experimental_workspace_rewind: group.experimental_workspace_rewind,
                permission_mode: group.permission_mode.or(permission_mode),
                edit_tool: group.edit_tool,
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
struct PartialDisplayConfig {
    show_reasoning_output: Option<bool>,
    zen_mode: Option<bool>,
    theme: Option<String>,
    max_tool_output_lines: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialOutputConfig {
    max_output_bytes: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialCompactionConfig {
    auto_compact: Option<bool>,
    compact_threshold_percent: Option<u8>,
    compact_target_percent: Option<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialInternalAgentModelConfig {
    /// `rho` (default) or `claude-cli`. Absent means Rho, so entries written
    /// before the Claude Code runtime existed keep loading unchanged.
    runtime: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    auth: Option<String>,
    reasoning: Option<ReasoningLevel>,
}

/// Builds one internal-agent selection, filling Rho defaults from the
/// conversation model.
///
/// An unusable `runtime` value falls back to Rho rather than failing the whole
/// config load, and Rho-only keys on a `claude-cli` entry are reported instead
/// of silently dropped.
fn internal_agent_selection(
    id: &str,
    group: PartialInternalAgentModelConfig,
    cfg: &Config,
    warnings: &mut Vec<ConfigWarning>,
) -> InternalAgentModelConfig {
    let runtime = match group.runtime.as_deref() {
        None | Some("rho") => InternalAgentRuntimeKey::Rho,
        Some(crate::config::CLAUDE_CLI_RUNTIME_KEY)
            if crate::agent::internal_agent_accepts_claude_runtime(id) =>
        {
            InternalAgentRuntimeKey::ClaudeCli
        }
        Some(crate::config::CLAUDE_CLI_RUNTIME_KEY) => {
            warnings.push(ConfigWarning::Normalized {
                key: "internal_agents.runtime",
                from: format!("\"{}\"", crate::config::CLAUDE_CLI_RUNTIME_KEY),
                to: format!("\"rho\"; internal agent '{id}' cannot delegate"),
            });
            InternalAgentRuntimeKey::Rho
        }
        Some(other) => {
            warnings.push(ConfigWarning::Normalized {
                key: "internal_agents.runtime",
                from: format!("\"{other}\""),
                to: "\"rho\"".into(),
            });
            InternalAgentRuntimeKey::Rho
        }
    };
    match runtime {
        InternalAgentRuntimeKey::Rho => {
            let provider = group.provider.unwrap_or_else(|| cfg.provider.clone());
            let auth = group
                .auth
                .unwrap_or_else(|| inferred_provider_auth(&provider, &cfg.provider, &cfg.auth));
            let mut selection = InternalAgentModelConfig::new(
                provider,
                group.model.unwrap_or_else(|| cfg.model.clone()),
                auth,
            );
            selection.reasoning = group.reasoning;
            selection
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
                        to: "no value; runtime claude-cli has no Rho provider or auth".into(),
                    });
                }
            }
            let mut selection = InternalAgentModelConfig::claude_cli(group.model);
            selection.reasoning = group.reasoning;
            selection
        }
    }
}

/// Parsed `runtime` key for an internal-agent entry.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InternalAgentRuntimeKey {
    Rho,
    ClaudeCli,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialTitleConfig {
    provider: Option<String>,
    model: Option<String>,
    auth: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialWebSearchConfig {
    hosted: Option<bool>,
    provider: Option<String>,
    openai_api_key: Option<String>,
    exa_api_key: Option<String>,
    brave_api_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialBehaviorConfig {
    check_for_updates: Option<bool>,
    enable_subagents: Option<bool>,
    advisor_mode: Option<bool>,
    experimental_workspace_rewind: Option<bool>,
    #[serde(default)]
    permission_mode: Option<PermissionMode>,
    edit_tool: Option<EditTool>,
    credential_store: Option<String>,
    rtk: Option<bool>,
    inline_shell: Option<String>,
}

#[cfg(test)]
#[path = "config_load_tests.rs"]
mod tests;
