use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use {
    crate::compaction::CompactionConfig, crate::model_aliases::ModelAliases, crate::paths,
    crate::permission::PermissionMode, crate::tools::mcp::config::McpConfig,
    rho_providers::credentials::CredentialStoreBackend, rho_providers::model::catalog,
    rho_providers::provider, rho_providers::reasoning::ReasoningLevel,
};

#[path = "provider_config.rs"]
mod provider_config;
pub(crate) use provider_config::ensure_serves_chat;

#[path = "config_streaming.rs"]
mod streaming;
pub use streaming::StreamingMode;

#[path = "config_internal_agent.rs"]
mod internal_agent;
pub use internal_agent::{
    InternalAgentModelConfig, InternalAgentTarget, ModelKind, RhoInternalAgentModel,
};

#[path = "config_document.rs"]
mod document;
pub(crate) use document::ConfigDocument;

#[path = "config_setting.rs"]
mod setting;
pub(crate) use setting::{
    any_value, load_settings, unknown_keys, AnySetting, Apply, ConfigError, IntegerSpec, Setting,
    SettingMeta,
};

#[cfg(test)]
pub(crate) use setting::default_fits_kind;

#[path = "config_registry.rs"]
mod registry;

#[path = "config_migrate_2x.rs"]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "install and upgrade call it from #1666")
)]
mod migrate_2x;

/// `runtime` value that selects the Claude Code CLI for an internal agent.
/// Matches the agent frontmatter vocabulary (`runtime: claude-cli`).
pub(crate) const CLAUDE_CLI_RUNTIME_KEY: &str = "claude-cli";
pub(crate) const RHO_RUNTIME_KEY: &str = "rho";
pub(crate) const CURSOR_RUNTIME_KEY: &str = "cursor";

/// The conversation model a fresh install starts on.
pub(crate) const DEFAULT_PROVIDER: &str = "openai";
pub(crate) const DEFAULT_MODEL: &str = "gpt-5.5";
pub(crate) const DEFAULT_AUTH: &str = "api-key";

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveModelConfig {
    pub provider: String,
    pub model: String,
    pub auth: String,
    pub source: EffectiveModelSource,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectiveModelSource {
    Conversation,
    Override,
}

/// Non-fatal issue found while loading config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConfigWarning {
    Normalized {
        key: &'static str,
        from: String,
        to: String,
    },
    /// A `[providers.custom.<name>]` table for the same host as a built-in
    /// provider that later took its name. The built-in serves it now, so the
    /// table is skipped.
    ShadowedCustomProvider { name: String },
}

impl ConfigWarning {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Normalized { key, from, to } => {
                format!("config `{key}` value {from} is unsupported; using {to}")
            }
            Self::ShadowedCustomProvider { name } => format!(
                "config `providers.custom.{name}` is ignored: {name} is now a built-in provider; remove the table"
            ),
        }
    }
}

#[path = "config_web_search.rs"]
mod web_search;
pub(crate) use web_search::{
    parse_search_endpoint_url, web_search_route, EXA_MCP_DEFAULT_URL, OPENAI_CODEX_RESPONSES_URL,
};
pub use web_search::{
    ExaSearchConnection, OpenAiSearchConnection, SearchBackend, WebSearchMode, WebSearchRoute,
    WebSearchSettings,
};

#[path = "config_load.rs"]
mod load;

pub(crate) use provider_config::ProviderConfigs;

/// Keep in lockstep with [`rho_tools::DEFAULT_MAX_OUTPUT_BYTES`].
pub(crate) const DEFAULT_MAX_OUTPUT_BYTES: usize = rho_tools::DEFAULT_MAX_OUTPUT_BYTES;
/// Default live delegated-agent slots.
///
/// Receipt: 4 serialized a 6-way background fan-out behind a permit queue.
/// 10 is the asked default; each slot is a full child agent, so this stays a
/// tripwire rather than a hardware measurement.
pub const DEFAULT_AGENT_CONCURRENCY: usize = 10;
/// Named ceiling for `behavior.agent_concurrency`.
///
/// Receipt: generous versus the default of 10. A typo like 100000 must fail at
/// check time instead of spawning. Raise this if a real workload hits it.
pub const MAX_AGENT_CONCURRENCY: usize = 64;

/// Persisted application configuration owned by `rho-coding-agent`.
///
/// This type is not part of the SDK contract. Convert it through
/// `app::sdk_config::SdkBootstrapOptions`, then acquire credentials separately
/// through the application credential adapter. Credentials are never stored
/// in these fields.
///
/// `config_registry` maps each setting to its `config.toml` key; the
/// structured tables (`providers`, `mcp`, ...) load in `config_load`. Client
/// display settings live with their client, not here.
#[derive(Clone, Debug)]
pub struct Config {
    pub questionnaire: QuestionnaireConfig,
    pub codemode: CodemodeConfig,
    pub provider: String,
    pub model: String,
    /// User-defined short names for concrete models; see `ModelAliases`.
    pub model_aliases: ModelAliases,
    /// Alias the current `provider`/`model` was resolved from, if any.
    /// Consult it through `current_model_alias`, which drops it once the
    /// selection no longer matches the alias table.
    pub model_alias: Option<String>,
    pub max_output_bytes: usize,
    pub auth: String,
    pub reasoning: ReasoningLevel,
    /// Faster serving for models that support `/fast`.
    ///
    /// Codex sends a priority service tier. xAI OAuth `grok-4.7` keeps that
    /// model selected and sends `grok-4.7-build-fast`.
    pub fast_mode: bool,
    pub auto_compact: bool,
    pub compact_threshold_percent: u8,
    pub compact_target_percent: u8,
    /// Optional model selections for reserved internal agents, keyed by stable agent ID.
    pub internal_agents: BTreeMap<String, InternalAgentModelConfig>,
    pub favorite_models: Vec<String>,
    /// Web-search mode, selected backend, and per-backend endpoints.
    pub web_search: WebSearchSettings,
    /// Attach xAI's hosted image generation tool on xAI create turns.
    pub xai_image_generation: bool,
    /// Selects the preferred built-in file edit tool exposed to models.
    ///
    /// [`EditTool::Auto`] resolves a concrete format from the active provider.
    pub edit_tool: EditTool,
    pub enable_subagents: bool,
    /// Maximum delegated agents that may execute at once, including background.
    pub agent_concurrency: usize,
    /// Offer the `advisor` tool, which reviews the session with the model
    /// configured for the `advisor` internal agent.
    pub advisor_mode: bool,
    /// Enables native-tool workspace checkpoints and `/rewind` (on by default).
    pub workspace_rewind: bool,
    /// Offer `keep_warm` on `process start`, letting the model keep the
    /// provider prompt cache warm while it waits on a long background command.
    pub keep_cache_warm: bool,
    pub permission_mode: PermissionMode,
    /// Explicit credential backend. `None` means unset; runtime defaults to OS.
    pub credential_store: Option<CredentialStoreBackend>,
    pub rtk: bool,
    pub inline_shell: String,
    pub prompt_templates: crate::prompt_templates::PromptTemplates,
    pub(crate) providers: ProviderConfigs,
    pub(crate) mcp: McpConfig,
    pub(crate) openapi: crate::tools::openapi::config::OpenApiConfig,
}

pub(crate) fn default_inline_shell() -> String {
    if cfg!(windows) { "powershell" } else { "bash" }.into()
}

pub(super) fn inferred_provider_auth(
    provider: &str,
    current_provider: &str,
    current_auth: &str,
) -> String {
    if provider == current_provider {
        return current_auth.into();
    }
    provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.default_auth().id.into())
        .unwrap_or_else(|| current_auth.into())
}

impl Default for Config {
    fn default() -> Self {
        let compaction = CompactionConfig::default();
        Self {
            questionnaire: QuestionnaireConfig::default(),
            codemode: CodemodeConfig::default(),
            provider: DEFAULT_PROVIDER.into(),
            model: DEFAULT_MODEL.into(),
            model_aliases: ModelAliases::default(),
            model_alias: None,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            auth: DEFAULT_AUTH.into(),
            reasoning: ReasoningLevel::Medium,
            fast_mode: false,
            auto_compact: compaction.auto_compact,
            compact_threshold_percent: compaction.threshold_percent,
            compact_target_percent: compaction.target_percent,
            internal_agents: BTreeMap::new(),
            favorite_models: Vec::new(),
            web_search: WebSearchSettings::default(),
            xai_image_generation: true,
            edit_tool: EditTool::default(),
            enable_subagents: true,
            agent_concurrency: DEFAULT_AGENT_CONCURRENCY,
            advisor_mode: false,
            workspace_rewind: true,
            keep_cache_warm: true,
            permission_mode: PermissionMode::Bypass,
            credential_store: None,
            rtk: true,
            inline_shell: default_inline_shell(),
            prompt_templates: Default::default(),
            providers: ProviderConfigs::default(),
            mcp: McpConfig::default(),
            openapi: Default::default(),
        }
    }
}

/// `[codemode]` settings, mirroring Pi's `codemode.mode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodemodeConfig {
    #[serde(default)]
    pub mode: CodemodeMode,
}

/// How the always-registered `codemode` tool presents other tools (Pi's
/// `codemode.mode`). Neither mode changes authorization: nested calls follow
/// `permission_mode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodemodeMode {
    /// Direct tools stay declared next to `codemode`; the model may use either.
    #[default]
    On,
    /// Direct tools are hidden by default and reached through `codemode`, unless
    /// top-level `tool_search` explicitly loads them for direct calls.
    Only,
}

impl CodemodeMode {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "on" => Some(Self::On),
            "only" => Some(Self::Only),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Only => "only",
        }
    }
}

/// User-owned timeout policy. Absence disables automatic questionnaire answers.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionnaireConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<std::num::NonZeroU64>,
}

/// Configured file-edit preference.
///
/// [`Self::Auto`] picks the preferred format for the active chat provider: a
/// custom provider's `edit_tool` override if set, otherwise the built-in
/// table. [`Self::Pinned`] freezes one [`rho_tools::EditFormat`] across
/// provider changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditTool {
    /// Prefer the built-in format for the active provider.
    #[default]
    Auto,
    /// Pin one concrete model-facing edit format.
    Pinned(rho_tools::EditFormat),
}

impl EditTool {
    /// Every supported preference, in UI display order.
    ///
    /// Pinned variants track [`rho_tools::EditFormat::ALL`] so newly added
    /// formats appear here without a second hard-coded list.
    pub fn all() -> Vec<Self> {
        let mut all = Vec::with_capacity(1 + rho_tools::EditFormat::ALL.len());
        all.push(Self::Auto);
        all.extend(rho_tools::EditFormat::ALL.iter().copied().map(Self::Pinned));
        all
    }

    /// Configured value and selector label (`behavior.edit_tool`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Pinned(format) => format.as_str(),
        }
    }

    /// Short human-readable preference label.
    pub const fn label(self) -> &'static str {
        self.as_str()
    }

    /// Detail shown when selecting an edit preference.
    pub fn detail(self) -> &'static str {
        match self {
            Self::Auto => {
                "Pick the preferred format for the active provider and switch when the provider changes."
            }
            Self::Pinned(format) => format.detail(),
        }
    }
}

impl Config {
    /// Resolves the edit preference for the configured provider.
    pub(crate) fn resolved_edit_tool(&self) -> rho_tools::EditFormat {
        self.resolved_edit_tool_for_provider(&self.provider)
    }

    /// Resolves the configured edit preference for a provider, including its
    /// custom `auto` override. This explicit form supports a live provider that
    /// differs from the last config saved to disk.
    pub(crate) fn resolved_edit_tool_for_provider(&self, provider: &str) -> rho_tools::EditFormat {
        match self.edit_tool {
            EditTool::Auto => self
                .providers
                .auto_edit_format(provider)
                .unwrap_or_else(|| preferred_edit_format_for_provider(provider)),
            EditTool::Pinned(format) => format,
        }
    }

    pub(crate) fn edit_tool_display_label(&self) -> String {
        self.edit_tool_display_label_for_provider(&self.provider)
    }

    pub(crate) fn edit_tool_display_label_for_provider(&self, provider: &str) -> String {
        match self.edit_tool {
            EditTool::Auto => format!("auto ({})", self.resolved_edit_tool_for_provider(provider)),
            EditTool::Pinned(format) => format.label().into(),
        }
    }
}

/// Built-in preferred edit format for a chat provider.
///
/// Product defaults, not user config. Many models train inside a first-party
/// harness that supplies one edit tool, so Auto picks that familiar surface.
/// Pin a concrete [`EditTool`] when a session should ignore this table.
/// Unknown providers fall back to str_replace.
pub fn preferred_edit_format_for_provider(provider: &str) -> rho_tools::EditFormat {
    match provider {
        // Codex harness trains on apply_patch documents.
        "openai-codex" => rho_tools::EditFormat::ApplyPatch,
        // Claude Code and xAI first-party agent tooling train on string replace.
        "anthropic" | "xai" => rho_tools::EditFormat::StrReplace,
        // Most reliable surface when no first-party match is known.
        _ => rho_tools::EditFormat::StrReplace,
    }
}

impl fmt::Display for EditTool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for EditTool {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.trim().to_ascii_lowercase();
        if normalized == "auto" {
            return Ok(Self::Auto);
        }
        if let Some(format) = rho_tools::EditFormat::from_config_value(&normalized) {
            return Ok(Self::Pinned(format));
        }
        let expected = Self::all()
            .into_iter()
            .map(Self::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "unknown edit tool {normalized:?}; expected {expected}"
        ))
    }
}

impl Serialize for EditTool {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EditTool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl Config {
    pub fn default_path() -> anyhow::Result<PathBuf> {
        Ok(paths::rho_dir()?.join("config.toml"))
    }

    /// Reads `config.toml`. A missing file is the defaults; reading never
    /// writes. Every bad key is reported, not just the first.
    pub fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let path = path.map(Ok).unwrap_or_else(Self::default_path)?;
        let doc = ConfigDocument::read(&path)?;
        let (config, warnings) = load::resolve(&doc).map_err(|errors| {
            load::errors_error(&errors).context(format!("invalid config file {}", path.display()))
        })?;
        load::emit_warnings(&path.display().to_string(), &warnings);
        Ok(config)
    }

    pub(crate) fn parse_settings(text: &str) -> anyhow::Result<Self> {
        Ok(Self::parse_settings_with_warnings(text)?.0)
    }

    pub(crate) fn parse_settings_with_warnings(
        text: &str,
    ) -> anyhow::Result<(Self, Vec<ConfigWarning>)> {
        load::resolve(&ConfigDocument::parse(text)?).map_err(|errors| load::errors_error(&errors))
    }

    /// Applies `edit` to the config saved at `path` and writes back only the
    /// keys whose values changed, keeping the rest of the file as written.
    ///
    /// Holds the file's edit lock throughout and fails without waiting when
    /// another writer holds it. Nothing is written when the file does not
    /// load before the edit, or would not load after it.
    pub(crate) fn edit<T>(path: &Path, edit: impl FnOnce(&mut Config) -> T) -> anyhow::Result<T> {
        let _lock = crate::config_writer::edit_lock::acquire_edit_lock(path)
            .with_context(|| format!("could not save config file {}", path.display()))?;
        let mut doc = ConfigDocument::read(path)?;
        let original = doc.text();
        let invalid = |errors: Vec<ConfigError>| {
            load::errors_error(&errors).context(format!("invalid config file {}", path.display()))
        };
        let (before, _) = load::resolve(&doc).map_err(invalid)?;
        let mut after = before.clone();
        let value = edit(&mut after);
        load::save_diff(&before, &after, &mut doc)?;
        load::resolve(&doc)
            .map_err(|errors| load::errors_error(&errors).context("the change was not saved"))?;
        if doc.text() != original {
            doc.write(path)?;
        }
        Ok(value)
    }

    /// Saves every value of this config that differs from the file, keeping
    /// the rest of the file as written. Production writers go through
    /// [`Self::edit`] or `ConfigRepository`.
    #[cfg(test)]
    pub(crate) fn save(&self, path: Option<PathBuf>) -> anyhow::Result<()> {
        let path = path.map(Ok).unwrap_or_else(Self::default_path)?;
        Self::edit(&path, |saved| *saved = self.clone())
    }

    fn validate_model_aliases(&self) -> anyhow::Result<()> {
        let implemented_providers = catalog::implemented_providers();
        for (name, target) in self.model_aliases.iter() {
            let Some(provider) = target.provider.as_deref() else {
                continue;
            };
            if !implemented_providers.contains(&provider)
                && !self.providers.custom.contains_key(provider)
            {
                anyhow::bail!("model alias '{name}' targets unknown provider '{provider}'");
            }
        }
        Ok(())
    }

    /// Resolve the configured session model reference to its concrete target.
    ///
    /// Runs once at load time, before any model-specific behavior, so every
    /// downstream consumer sees only concrete model ids.
    fn resolve_model_alias(&mut self) -> anyhow::Result<()> {
        let resolved = self
            .model_aliases
            .resolve(&self.model)
            .map_err(|error| anyhow::anyhow!("session model: {error}"))?;
        self.model_alias = resolved.alias;
        if let Some(provider) = resolved
            .provider
            .as_deref()
            .filter(|provider| *provider != self.provider)
        {
            if let Some(descriptor) = provider::provider_descriptor(provider) {
                self.auth = descriptor.default_auth().id.into();
            }
            self.provider = provider.to_string();
        }
        self.model = resolved.model;
        Ok(())
    }

    /// Resolves `@alias` model references for internal agents.
    ///
    /// Only Rho selections take part: the Claude Code runtime passes its model
    /// straight to `--model` and never resolves a Rho alias, the same rule the
    /// agent parser applies to `runtime: claude-cli` definitions.
    fn resolve_internal_agent_model_aliases(&mut self) -> anyhow::Result<()> {
        for (id, selection) in &mut self.internal_agents {
            match &mut selection.target {
                InternalAgentTarget::Rho(rho) => {
                    let resolved = self
                        .model_aliases
                        .resolve(&rho.model)
                        .map_err(|error| anyhow::anyhow!("internal agent '{id}' model: {error}"))?;
                    rho.model_alias = resolved.alias;
                    if let Some(provider) = resolved.provider {
                        if rho.provider != provider {
                            if let Some(descriptor) = provider::provider_descriptor(&provider) {
                                rho.auth = descriptor.default_auth().id.into();
                            }
                            rho.provider = provider;
                        }
                    }
                    rho.model = resolved.model;
                }
                InternalAgentTarget::ClaudeCli { .. } => continue,
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn effective_internal_agent_model(&self, id: &str) -> EffectiveModelConfig {
        match self.internal_agents.get(id) {
            Some(selection) => {
                let rho = selection.expect_rho();
                EffectiveModelConfig {
                    provider: rho.provider.clone(),
                    model: rho.model.clone(),
                    auth: rho.auth.clone(),
                    source: EffectiveModelSource::Override,
                }
            }
            None => EffectiveModelConfig {
                provider: self.provider.clone(),
                model: self.model.clone(),
                auth: self.auth.clone(),
                source: EffectiveModelSource::Conversation,
            },
        }
    }

    #[cfg(test)]
    pub fn set_internal_agent_model(
        &mut self,
        id: impl Into<String>,
        provider: String,
        model: String,
        auth: String,
    ) {
        self.set_internal_agent_model_config(
            id,
            InternalAgentModelConfig::new(provider, model, auth),
        );
    }

    pub fn set_internal_agent_model_config(
        &mut self,
        id: impl Into<String>,
        selection: InternalAgentModelConfig,
    ) {
        self.internal_agents.insert(id.into(), selection);
    }

    pub fn clear_internal_agent_model(&mut self, id: &str) {
        self.internal_agents.remove(id);
    }

    /// The model explicitly configured for an internal agent, with no
    /// conversation-model fallback. Agents that need their own model
    /// (see `internal_agent_requires_model`) read this instead of
    /// `effective_internal_agent_model`.
    pub fn internal_agent_model(&self, id: &str) -> Option<&InternalAgentModelConfig> {
        self.internal_agents.get(id)
    }

    #[cfg(test)]
    pub fn current_internal_agent_model_alias(&self, id: &str) -> Option<&str> {
        self.internal_agents
            .get(id)?
            .current_alias(&self.model_aliases)
    }

    /// The alias behind the current model selection, provided the alias table
    /// still maps it there; stale aliases silently drop out.
    pub fn current_model_alias(&self) -> Option<&str> {
        let name = self.model_alias.as_deref()?;
        let target = self.model_aliases.get(name)?;
        (target.model == self.model
            && target.provider.as_deref().unwrap_or(&self.provider) == self.provider)
            .then_some(name)
    }
}

impl From<&Config> for CompactionConfig {
    fn from(config: &Config) -> Self {
        let summarizer = config
            .internal_agent_model(crate::agent::COMPACTION_AGENT_ID)
            .and_then(|selection| {
                // The compaction agent cannot delegate, so config load keeps
                // every selection on Rho's stack.
                let rho = selection.rho()?;
                Some(crate::compaction::SummarizerModel {
                    provider: rho.provider.clone(),
                    model: rho.model.clone(),
                    auth: rho.auth.clone(),
                    reasoning: crate::agent::effective_internal_agent_reasoning(
                        crate::agent::COMPACTION_AGENT_ID,
                        selection,
                    ),
                })
            });
        Self {
            auto_compact: config.auto_compact,
            threshold_percent: config.compact_threshold_percent,
            target_percent: config.compact_target_percent,
            summarizer,
        }
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
