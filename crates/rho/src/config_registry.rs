//! Every agent setting `config.toml` holds, declared once.
//!
//! This table is the only map between [`Config`] fields and their TOML keys
//! (which are also their protocol keys). Loading, saving, and the settings
//! schema all walk [`AGENT_SETTINGS`]; the structured tables that stay
//! hand-edited are listed in `config_load::FILE_ONLY`.

use std::num::NonZeroU64;

use rho_protocol::{IntegerUnit, ModelRuntime};

use super::setting::{
    any_value, AnySetting, Apply, IntegerSpec, ModelSetting, ModelSpec, Setting, SettingMeta,
};
use super::{
    CodemodeMode, Config, EditTool, ExaSearchConnection, InternalAgentModelConfig,
    InternalAgentTarget, OpenAiSearchConnection, SearchBackend, WebSearchMode,
};
use {
    crate::agent::{ADVISOR_AGENT_ID, PERMISSION_CLASSIFIER_AGENT_ID},
    crate::permission::PermissionMode,
    rho_providers::credentials::CredentialStoreBackend,
    rho_providers::reasoning::ReasoningLevel,
};

/// Every agent setting, in schema order.
pub(crate) static AGENT_SETTINGS: &[&dyn AnySetting<Config>] = &[
    &MODEL,
    &REASONING,
    &FAST_MODE,
    &PERMISSION_MODE,
    &PERMISSION_CLASSIFIER_MODEL,
    &PERMISSION_CLASSIFIER_REASONING,
    &ADVISOR_MODE,
    &ADVISOR_MODEL,
    &ADVISOR_REASONING,
    &ENABLE_SUBAGENTS,
    &AGENT_CONCURRENCY,
    &QUESTIONNAIRE_TIMEOUT,
    &WORKSPACE_REWIND,
    &KEEP_CACHE_WARM,
    &AUTO_COMPACT,
    &COMPACT_THRESHOLD,
    &COMPACT_TARGET,
    &MAX_OUTPUT_BYTES,
    &INLINE_SHELL,
    &EDIT_TOOL,
    &RTK,
    &CODEMODE,
    &WEB_SEARCH_MODE,
    &WEB_SEARCH_BACKEND,
    &OPENAI_SEARCH_CONNECTION,
    &OPENAI_SEARCH_URL,
    &EXA_SEARCH_CONNECTION,
    &EXA_SEARCH_URL,
    &EXA_MCP_URL,
    &BRAVE_SEARCH_URL,
    &FIRECRAWL_SEARCH_URL,
    &XAI_IMAGE_GENERATION,
    &CREDENTIAL_STORE,
];

const fn meta(
    category: &'static str,
    section: Option<&'static str>,
    label: &'static str,
    help: &'static str,
    applies: Apply,
) -> SettingMeta {
    SettingMeta {
        category,
        section,
        label,
        help,
        applies,
        overridable_per_project: true,
    }
}

/// A setting that sizes or wires the daemon itself, so a project never
/// overrides it.
const fn daemon_wide(meta: SettingMeta) -> SettingMeta {
    SettingMeta {
        overridable_per_project: false,
        ..meta
    }
}

const RHO_ONLY: ModelSpec = ModelSpec {
    runtimes: &[ModelRuntime::Rho],
};
const RHO_OR_CLAUDE_CLI: ModelSpec = ModelSpec {
    runtimes: &[ModelRuntime::Rho, ModelRuntime::ClaudeCli],
};

/// Percent of the effective context window; the loader clamped to this
/// range before 3.0.
const PERCENT: IntegerSpec = IntegerSpec {
    min: 1,
    max: Some(100),
    unit: Some(IntegerUnit::Percent),
};

static MODEL: Setting<Config, ModelSetting> = Setting {
    key: "model",
    meta: meta(
        "models",
        None,
        "Conversation model",
        "Model used for conversation turns.",
        Apply::NextSession,
    ),
    spec: RHO_ONLY,
    get: |config| ModelSetting::Rho {
        provider: config.provider.clone(),
        model: match config.current_model_alias() {
            Some(alias) => format!("@{alias}"),
            None => config.model.clone(),
        },
        auth: config.auth.clone(),
    },
    set: |config, model| match model {
        ModelSetting::Rho {
            provider,
            model,
            auth,
        } => {
            config.provider = provider;
            config.model = model;
            config.auth = auth;
            // A chosen model replaces the alias it was resolved from;
            // loading resolves an `@alias` model again.
            config.model_alias = None;
        }
        // RHO_ONLY keeps the reader from producing one.
        ModelSetting::ClaudeCli { .. } => {}
    },
    validate: any_value,
};

static REASONING: Setting<Config, ReasoningLevel> = Setting {
    key: "model.reasoning",
    meta: meta(
        "models",
        None,
        "Reasoning",
        "How much the conversation model reasons on each turn.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.reasoning,
    set: |config, level| config.reasoning = level,
    validate: any_value,
};

static FAST_MODE: Setting<Config, bool> = Setting {
    key: "model.fast_mode",
    meta: meta(
        "models",
        None,
        "Fast mode",
        "Faster serving for models that offer it, at a higher credit rate.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.fast_mode,
    set: |config, enabled| config.fast_mode = enabled,
    validate: any_value,
};

static PERMISSION_MODE: Setting<Config, PermissionMode> = Setting {
    key: "behavior.permission_mode",
    meta: meta(
        "agent",
        Some("Permissions"),
        "Permission mode",
        "Which tool calls run freely, which ask, and which are denied.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.permission_mode,
    set: |config, mode| config.permission_mode = mode,
    validate: any_value,
};

static PERMISSION_CLASSIFIER_MODEL: Setting<Config, Option<ModelSetting>> = Setting {
    key: "internal_agents.permission-classifier",
    meta: meta(
        "agent",
        Some("Permissions"),
        "Permission classifier model",
        "Model Auto permission mode uses to review writes, processes, and reads outside the workspace.",
        Apply::NextSession,
    ),
    spec: RHO_OR_CLAUDE_CLI,
    get: |config| internal_agent_model(config, PERMISSION_CLASSIFIER_AGENT_ID),
    set: |config, model| set_internal_agent_model(config, PERMISSION_CLASSIFIER_AGENT_ID, model),
    validate: any_value,
};

static PERMISSION_CLASSIFIER_REASONING: Setting<Config, Option<ReasoningLevel>> = Setting {
    key: "internal_agents.permission-classifier.reasoning",
    meta: meta(
        "agent",
        Some("Permissions"),
        "Permission classifier reasoning",
        "How much the permission classifier model reasons; unset keeps the agent's default.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| internal_agent_reasoning(config, PERMISSION_CLASSIFIER_AGENT_ID),
    set: |config, level| {
        set_internal_agent_reasoning(config, PERMISSION_CLASSIFIER_AGENT_ID, level)
    },
    validate: any_value,
};

static ADVISOR_MODE: Setting<Config, bool> = Setting {
    key: "behavior.advisor_mode",
    meta: meta(
        "agent",
        Some("Advisor"),
        "Advisor mode",
        "Let the agent ask an advisor model to review the session. Needs an advisor model.",
        Apply::NextTurn,
    ),
    spec: (),
    get: |config| config.advisor_mode,
    set: |config, enabled| config.advisor_mode = enabled,
    validate: any_value,
};

static ADVISOR_MODEL: Setting<Config, Option<ModelSetting>> = Setting {
    key: "internal_agents.advisor",
    meta: meta(
        "agent",
        Some("Advisor"),
        "Advisor model",
        "Model the advisor tool asks.",
        Apply::NextTurn,
    ),
    spec: RHO_OR_CLAUDE_CLI,
    get: |config| internal_agent_model(config, ADVISOR_AGENT_ID),
    set: |config, model| set_internal_agent_model(config, ADVISOR_AGENT_ID, model),
    validate: any_value,
};

static ADVISOR_REASONING: Setting<Config, Option<ReasoningLevel>> = Setting {
    key: "internal_agents.advisor.reasoning",
    meta: meta(
        "agent",
        Some("Advisor"),
        "Advisor reasoning",
        "How much the advisor model reasons; unset keeps the agent's default.",
        Apply::NextTurn,
    ),
    spec: (),
    get: |config| internal_agent_reasoning(config, ADVISOR_AGENT_ID),
    set: |config, level| set_internal_agent_reasoning(config, ADVISOR_AGENT_ID, level),
    validate: any_value,
};

static ENABLE_SUBAGENTS: Setting<Config, bool> = Setting {
    key: "behavior.enable_subagents",
    meta: meta(
        "agent",
        Some("Subagents"),
        "Delegation",
        "Make agent tools available.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.enable_subagents,
    set: |config, enabled| config.enable_subagents = enabled,
    validate: any_value,
};

static AGENT_CONCURRENCY: Setting<Config, u64> = Setting {
    key: "behavior.agent_concurrency",
    meta: daemon_wide(meta(
        "agent",
        Some("Subagents"),
        "Concurrent agents",
        "Maximum delegated agents running at once, including background ones.",
        Apply::Immediately,
    )),
    // Receipt for the ceiling: `MAX_AGENT_CONCURRENCY`.
    spec: IntegerSpec {
        min: 1,
        max: Some(super::MAX_AGENT_CONCURRENCY as u64),
        unit: None,
    },
    get: |config| config.agent_concurrency as u64,
    set: |config, slots| config.agent_concurrency = slots as usize,
    validate: any_value,
};

static QUESTIONNAIRE_TIMEOUT: Setting<Config, Option<u64>> = Setting {
    key: "questionnaire.timeout_seconds",
    meta: meta(
        "agent",
        Some("Questions"),
        "Questionnaire timeout",
        "Seconds before a form with explicit fallback answers takes them. Unset never times out; interaction pauses the timer.",
        Apply::NextSession,
    ),
    spec: IntegerSpec {
        min: 1,
        max: None,
        unit: Some(IntegerUnit::Seconds),
    },
    get: |config| config.questionnaire.timeout_seconds.map(NonZeroU64::get),
    set: |config, seconds| config.questionnaire.timeout_seconds = seconds.and_then(NonZeroU64::new),
    validate: any_value,
};

static WORKSPACE_REWIND: Setting<Config, bool> = Setting {
    key: "behavior.workspace_rewind",
    meta: meta(
        "agent",
        Some("Workspace"),
        "Workspace rewind",
        "Capture native file-tool changes for /rewind. Checkpoints hold file contents; shell and process effects are not captured.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.workspace_rewind,
    set: |config, enabled| config.workspace_rewind = enabled,
    validate: any_value,
};

static KEEP_CACHE_WARM: Setting<Config, bool> = Setting {
    key: "behavior.keep_cache_warm",
    meta: meta(
        "agent",
        Some("Workspace"),
        "Keep cache warm",
        "Let the model keep the prompt cache warm while it waits on a long background process. Each warm-up re-reads the cached prompt at the provider's cache-read price.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.keep_cache_warm,
    set: |config, enabled| config.keep_cache_warm = enabled,
    validate: any_value,
};

static AUTO_COMPACT: Setting<Config, bool> = Setting {
    key: "compaction.auto_compact",
    meta: meta(
        "context",
        None,
        "Auto compact",
        "Summarize older context before the effective context limit.",
        Apply::NextTurn,
    ),
    spec: (),
    get: |config| config.auto_compact,
    set: |config, enabled| config.auto_compact = enabled,
    validate: any_value,
};

static COMPACT_THRESHOLD: Setting<Config, u64> = Setting {
    key: "compaction.compact_threshold_percent",
    meta: meta(
        "context",
        None,
        "Compact threshold",
        "Percent of the effective context window that triggers auto compaction.",
        Apply::NextTurn,
    ),
    spec: PERCENT,
    get: |config| u64::from(config.compact_threshold_percent),
    set: |config, percent| config.compact_threshold_percent = percent as u8,
    validate: any_value,
};

static COMPACT_TARGET: Setting<Config, u64> = Setting {
    key: "compaction.compact_target_percent",
    meta: meta(
        "context",
        None,
        "Compact target",
        "Post-compaction target percent for text-summary compaction; must be below the threshold.",
        Apply::NextTurn,
    ),
    spec: PERCENT,
    get: |config| u64::from(config.compact_target_percent),
    set: |config, percent| config.compact_target_percent = percent as u8,
    validate: any_value,
};

static MAX_OUTPUT_BYTES: Setting<Config, u64> = Setting {
    key: "output.max_output_bytes",
    meta: meta(
        "context",
        None,
        "Max output bytes",
        "Maximum tool output retained in context.",
        Apply::NextSession,
    ),
    // The settings editor never accepted less than 1.
    spec: IntegerSpec {
        min: 1,
        max: None,
        unit: Some(IntegerUnit::Bytes),
    },
    get: |config| config.max_output_bytes as u64,
    set: |config, bytes| config.max_output_bytes = bytes as usize,
    validate: any_value,
};

static INLINE_SHELL: Setting<Config, String> = Setting {
    key: "behavior.inline_shell",
    meta: meta(
        "tools",
        None,
        "Inline shell",
        "Shell used by ! and !! commands.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.inline_shell.clone(),
    set: |config, shell| config.inline_shell = shell,
    validate: |shell| {
        if shell.trim().is_empty() {
            Err("must name a shell".into())
        } else {
            Ok(())
        }
    },
};

static EDIT_TOOL: Setting<Config, EditTool> = Setting {
    key: "behavior.edit_tool",
    meta: meta(
        "tools",
        None,
        "Edit tool",
        "File edit format exposed to models. Auto follows the active provider.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.edit_tool,
    set: |config, tool| config.edit_tool = tool,
    validate: any_value,
};

static RTK: Setting<Config, bool> = Setting {
    key: "behavior.rtk",
    meta: meta(
        "tools",
        None,
        "RTK",
        "Rewrite shell commands through rtk to compact their output, when rtk is installed.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.rtk,
    set: |config, enabled| config.rtk = enabled,
    validate: any_value,
};

static CODEMODE: Setting<Config, CodemodeMode> = Setting {
    key: "codemode.mode",
    meta: meta(
        "tools",
        None,
        "Codemode",
        "On keeps direct tools next to codemode; only hides them behind codemode.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.codemode.mode,
    set: |config, mode| config.codemode.mode = mode,
    validate: any_value,
};

static WEB_SEARCH_MODE: Setting<Config, WebSearchMode> = Setting {
    key: "web_search.mode",
    meta: meta(
        "tools",
        Some("Web search"),
        "Web search mode",
        "Auto uses the chat provider's own search when it has one, otherwise the backend.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.web_search.mode,
    set: |config, mode| config.web_search.mode = mode,
    validate: any_value,
};

static WEB_SEARCH_BACKEND: Setting<Config, SearchBackend> = Setting {
    key: "web_search.backend",
    meta: meta(
        "tools",
        Some("Web search"),
        "Search backend",
        "Service that answers searches Rho runs itself.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.web_search.backend,
    set: |config, backend| config.web_search.backend = backend,
    validate: any_value,
};

static OPENAI_SEARCH_CONNECTION: Setting<Config, OpenAiSearchConnection> = Setting {
    key: "web_search.openai.connection",
    meta: meta(
        "tools",
        Some("Web search"),
        "OpenAI search connection",
        "Reach OpenAI search through the OpenAI API or a Codex sign-in.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.web_search.openai.connection,
    set: |config, connection| config.web_search.openai.connection = connection,
    validate: any_value,
};

static OPENAI_SEARCH_URL: Setting<Config, Option<String>> = Setting {
    key: "web_search.openai.api_base_url",
    meta: meta(
        "tools",
        Some("Web search"),
        "OpenAI search endpoint",
        "API origin and path prefix for OpenAI search; unset uses OpenAI's.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.web_search.openai.api_base_url.clone(),
    set: |config, url| config.web_search.openai.api_base_url = url,
    validate: endpoint_url,
};

static EXA_SEARCH_CONNECTION: Setting<Config, ExaSearchConnection> = Setting {
    key: "web_search.exa.connection",
    meta: meta(
        "tools",
        Some("Web search"),
        "Exa search connection",
        "Reach Exa through its HTTP API or its MCP server.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.web_search.exa.connection,
    set: |config, connection| config.web_search.exa.connection = connection,
    validate: any_value,
};

static EXA_SEARCH_URL: Setting<Config, Option<String>> = Setting {
    key: "web_search.exa.api_base_url",
    meta: meta(
        "tools",
        Some("Web search"),
        "Exa API endpoint",
        "API origin and path prefix for Exa; unset uses Exa's.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.web_search.exa.api_base_url.clone(),
    set: |config, url| config.web_search.exa.api_base_url = url,
    validate: endpoint_url,
};

static EXA_MCP_URL: Setting<Config, Option<String>> = Setting {
    key: "web_search.exa.mcp_url",
    meta: meta(
        "tools",
        Some("Web search"),
        "Exa MCP endpoint",
        "MCP server URL for Exa; unset uses Exa's.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.web_search.exa.mcp_url.clone(),
    set: |config, url| config.web_search.exa.mcp_url = url,
    validate: endpoint_url,
};

static BRAVE_SEARCH_URL: Setting<Config, Option<String>> = Setting {
    key: "web_search.brave.api_base_url",
    meta: meta(
        "tools",
        Some("Web search"),
        "Brave API endpoint",
        "API origin and path prefix for Brave; unset uses Brave's.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.web_search.brave.api_base_url.clone(),
    set: |config, url| config.web_search.brave.api_base_url = url,
    validate: endpoint_url,
};

static FIRECRAWL_SEARCH_URL: Setting<Config, Option<String>> = Setting {
    key: "web_search.firecrawl.api_base_url",
    meta: meta(
        "tools",
        Some("Web search"),
        "Firecrawl API endpoint",
        "API origin and path prefix for Firecrawl; unset uses Firecrawl's.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.web_search.firecrawl.api_base_url.clone(),
    set: |config, url| config.web_search.firecrawl.api_base_url = url,
    validate: endpoint_url,
};

static XAI_IMAGE_GENERATION: Setting<Config, bool> = Setting {
    key: "xai.image_generation",
    meta: meta(
        "tools",
        None,
        "SpaceXAI image generation",
        "Attach SpaceXAI hosted image generation on xAI create turns.",
        Apply::NextSession,
    ),
    spec: (),
    get: |config| config.xai_image_generation,
    set: |config, enabled| config.xai_image_generation = enabled,
    validate: any_value,
};

/// Changed only through `rho/auth/choose_store`, which checks the store
/// can be reached; `rho/config/set` refuses it.
pub(super) static CREDENTIAL_STORE: Setting<Config, Option<CredentialStoreBackend>> = Setting {
    key: "behavior.credential_store",
    meta: daemon_wide(meta(
        "providers",
        None,
        "Credential store",
        "Where provider credentials live. Unset uses the OS keyring.",
        Apply::Restart,
    )),
    spec: (),
    get: |config| config.credential_store,
    set: |config, backend| config.credential_store = backend,
    validate: any_value,
};

fn endpoint_url(url: &Option<String>) -> Result<(), String> {
    match url {
        Some(url) => super::parse_search_endpoint_url("endpoint", url)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        None => Ok(()),
    }
}

fn internal_agent_model(config: &Config, id: &str) -> Option<ModelSetting> {
    let selection = config.internal_agents.get(id)?;
    Some(match &selection.target {
        InternalAgentTarget::Rho(rho) => ModelSetting::Rho {
            provider: rho.provider.clone(),
            model: match selection.current_alias(&config.model_aliases) {
                Some(alias) => format!("@{alias}"),
                None => rho.model.clone(),
            },
            auth: rho.auth.clone(),
        },
        InternalAgentTarget::ClaudeCli { model } => ModelSetting::ClaudeCli {
            model: model.clone(),
        },
    })
}

/// Replaces an internal agent's model, keeping its reasoning override.
fn set_internal_agent_model(config: &mut Config, id: &str, model: Option<ModelSetting>) {
    let Some(model) = model else {
        config.internal_agents.remove(id);
        return;
    };
    let reasoning = config
        .internal_agents
        .get(id)
        .and_then(|selection| selection.reasoning);
    let mut selection = match model {
        ModelSetting::Rho {
            provider,
            model,
            auth,
        } => InternalAgentModelConfig::new(provider, model, auth),
        ModelSetting::ClaudeCli { model } => InternalAgentModelConfig::claude_cli(model),
    };
    selection.reasoning = reasoning;
    config.internal_agents.insert(id.into(), selection);
}

fn internal_agent_reasoning(config: &Config, id: &str) -> Option<ReasoningLevel> {
    config.internal_agents.get(id)?.reasoning
}

/// Sets an internal agent's reasoning override. An agent without a model
/// has no entry to hold one, so the level waits for a model.
fn set_internal_agent_reasoning(config: &mut Config, id: &str, level: Option<ReasoningLevel>) {
    if let Some(selection) = config.internal_agents.get_mut(id) {
        selection.reasoning = level;
    }
}

#[cfg(test)]
#[path = "config_registry_tests.rs"]
mod tests;
