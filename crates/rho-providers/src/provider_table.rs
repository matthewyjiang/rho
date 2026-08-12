//! Static provider descriptor table.
//!
//! Runtime shape, default model, auth modes, and catalog policy live on each
//! row so adding a provider is a single table entry rather than parallel match
//! arms across the crate.

use super::{
    AuthMode, BearerCredentialAcquisition, BrowserOAuthFlow, CatalogReasoningPolicy, ModelIdCodec,
    OpenAiRuntimeAuth, ProviderAuthKind, ProviderDescriptor, ProviderId, ProviderModelRefreshKind,
    ProviderModelSource, ProviderRuntime, ANTHROPIC_API_KEY_ACCOUNT, CODEX_TOKENS_ACCOUNT,
    CURSOR_TOKENS_ACCOUNT, GITHUB_COPILOT_TOKENS_ACCOUNT, GOOGLE_API_KEY_ACCOUNT,
    KIMI_CODE_API_BASE, KIMI_TOKENS_ACCOUNT, META_API_BASE, META_API_KEY_ACCOUNT,
    MOONSHOT_API_BASE, MOONSHOT_API_KEY_ACCOUNT, OLLAMA_API_BASE, OLLAMA_CLOUD_API_BASE,
    OLLAMA_CLOUD_API_KEY_ACCOUNT, OPENAI_API_KEY_ACCOUNT, OPENROUTER_API_BASE,
    OPENROUTER_API_KEY_ACCOUNT, OPENROUTER_OAUTH_KEY_ACCOUNT, POOLSIDE_API_BASE,
    POOLSIDE_API_KEY_ACCOUNT, QWEN_TOKEN_PLAN_API_BASE, QWEN_TOKEN_PLAN_API_KEY_ACCOUNT,
    XAI_API_KEY_ACCOUNT, XAI_TOKENS_ACCOUNT,
};
use crate::openai_compatible_dialect::OpenAiCompatibleDialect;

pub const PROVIDERS: &[ProviderDescriptor] = &[
    ProviderDescriptor {
        id: ProviderId::Ollama,
        runtime: ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Standard,
            default_api_base: OLLAMA_API_BASE,
        },
        name: "ollama",
        display_name: "Ollama",
        auth_modes: &[
        AuthMode {
            id: "none",
            login_label: "No authentication required",
            auth_kind: ProviderAuthKind::None,
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "ollama",
        catalog_reasoning: CatalogReasoningPolicy::NotConfigurable,
        default_model: None,
    },
    ProviderDescriptor {
        id: ProviderId::OllamaCloud,
        runtime: ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Standard,
            default_api_base: OLLAMA_CLOUD_API_BASE,
        },
        name: "ollama-cloud",
        display_name: "Ollama Cloud",
        auth_modes: &[
        AuthMode {
            id: "ollama-cloud-api-key",
            login_label: "Ollama Cloud API key",
            auth_kind: ProviderAuthKind::ApiKey {
            env_var: "OLLAMA_API_KEY",
            account: OLLAMA_CLOUD_API_KEY_ACCOUNT,
            entry_label: "Ollama Cloud API key",
            missing_message: "missing Ollama Cloud API key; run /login ollama-cloud in the TUI or set OLLAMA_API_KEY as a CI/dev override",
        },
        },
        AuthMode {
            id: "ollama-cloud-device",
            login_label: "Ollama Cloud device key",
            auth_kind: ProviderAuthKind::OllamaDeviceKey {
                missing_message: "missing Ollama Cloud device key; run /login ollama-cloud in the TUI and choose Device Key, or sign in with `ollama signin` so ~/.ollama/id_ed25519 is registered",
            },
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        // models.dev catalogs cloud models under `ollama-cloud`, not `ollama`.
        metadata_upstream: "ollama-cloud",
        // Ollama's OpenAI-compatible API accepts reasoning_effort including "none".
        catalog_reasoning: CatalogReasoningPolicy::OffAsNone,
        default_model: None,
    },
    ProviderDescriptor {
        id: ProviderId::OpenAi,
        runtime: ProviderRuntime::OpenAi {
            auth_mode: OpenAiRuntimeAuth::ApiKey,
        },
        name: "openai",
        display_name: "OpenAI",
        auth_modes: &[
        AuthMode {
            id: "api-key",
            login_label: "OpenAI API key",
            auth_kind: ProviderAuthKind::ApiKey {
            env_var: "OPENAI_API_KEY",
            account: OPENAI_API_KEY_ACCOUNT,
            entry_label: "OpenAI API key",
            missing_message: "missing OpenAI API key; run /login openai in the TUI or set OPENAI_API_KEY as a CI/dev override",
        },
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAi),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "openai",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
        default_model: None,
    },
    ProviderDescriptor {
        id: ProviderId::OpenAiCodex,
        runtime: ProviderRuntime::OpenAi {
            auth_mode: OpenAiRuntimeAuth::Codex,
        },
        name: "openai-codex",
        display_name: "OpenAI Codex",
        auth_modes: &[
        AuthMode {
            id: "codex",
            login_label: "Codex OAuth",
            auth_kind: ProviderAuthKind::CodexOAuth {
            env_var: "CODEX_ACCESS_TOKEN",
            account: CODEX_TOKENS_ACCOUNT,
            missing_message: "missing Codex OAuth credentials; run /login openai-codex in the TUI or set CODEX_ACCESS_TOKEN as a CI/dev override",
        },
        }
        ],
        model_source: ProviderModelSource::StaticCatalog,
        model_refresh: None,
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "openai",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
        default_model: None,
    },
    ProviderDescriptor {
        id: ProviderId::Anthropic,
        runtime: ProviderRuntime::Anthropic,
        name: "anthropic",
        display_name: "Anthropic",
        auth_modes: &[
        AuthMode {
            id: "anthropic-api-key",
            login_label: "Anthropic API key",
            auth_kind: ProviderAuthKind::ApiKey {
            env_var: "ANTHROPIC_API_KEY",
            account: ANTHROPIC_API_KEY_ACCOUNT,
            entry_label: "Anthropic API key",
            missing_message: "missing Anthropic API key; run /login anthropic in the TUI or set ANTHROPIC_API_KEY as a CI/dev override",
        },
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::Anthropic),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "anthropic",
        catalog_reasoning: CatalogReasoningPolicy::Unknown,
        default_model: Some("claude-sonnet-4-5"),
    },
    ProviderDescriptor {
        id: ProviderId::Google,
        runtime: ProviderRuntime::Google,
        name: "google",
        display_name: "Google Gemini",
        auth_modes: &[
        AuthMode {
            id: "google-api-key",
            login_label: "Google Gemini API key",
            auth_kind: ProviderAuthKind::ApiKey {
            env_var: "GEMINI_API_KEY",
            account: GOOGLE_API_KEY_ACCOUNT,
            entry_label: "Google Gemini API key",
            missing_message: "missing Google Gemini API key; run /login google in the TUI or set GEMINI_API_KEY as a CI/dev override",
        },
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::Google),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "google",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
        default_model: Some("gemini-3.1-flash-lite"),
    },
    ProviderDescriptor {
        id: ProviderId::GithubCopilot,
        runtime: ProviderRuntime::GithubCopilot,
        name: "github-copilot",
        display_name: "GitHub Copilot",
        auth_modes: &[
        AuthMode {
            id: "github-copilot",
            login_label: "GitHub Copilot device login",
            auth_kind: ProviderAuthKind::GithubCopilotDevice {
            env_var: "GITHUB_COPILOT_TOKEN",
            account: GITHUB_COPILOT_TOKENS_ACCOUNT,
            missing_message: "missing GitHub Copilot credentials; run /login github-copilot in the TUI or set GITHUB_COPILOT_TOKEN as a CI/dev override",
        },
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::GithubCopilot),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "github-copilot",
        catalog_reasoning: CatalogReasoningPolicy::NotConfigurable,
        default_model: None,
    },
    ProviderDescriptor {
        id: ProviderId::Moonshot,
        runtime: ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Moonshot,
            default_api_base: MOONSHOT_API_BASE,
        },
        name: "moonshot",
        display_name: "Moonshot AI",
        auth_modes: &[
        AuthMode {
            id: "moonshot-api-key",
            login_label: "Moonshot API key",
            auth_kind: ProviderAuthKind::ApiKey {
            env_var: "MOONSHOT_API_KEY",
            account: MOONSHOT_API_KEY_ACCOUNT,
            entry_label: "Moonshot API key",
            missing_message: "missing Moonshot API key; run /login moonshot in the TUI or set MOONSHOT_API_KEY as a CI/dev override",
        },
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "moonshotai",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
        default_model: None,
    },
    ProviderDescriptor {
        id: ProviderId::Poolside,
        runtime: ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Poolside,
            default_api_base: POOLSIDE_API_BASE,
        },
        name: "poolside",
        display_name: "Poolside",
        auth_modes: &[
        AuthMode {
            id: "poolside-api-key",
            login_label: "Poolside API key",
            auth_kind: ProviderAuthKind::ApiKey {
            env_var: "POOLSIDE_API_KEY",
            account: POOLSIDE_API_KEY_ACCOUNT,
            entry_label: "Poolside API key",
            missing_message: "missing Poolside API key; run /login poolside in the TUI or set POOLSIDE_API_KEY as a CI/dev override",
        },
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::ProviderPrefixed,
        metadata_upstream: "poolside",
        catalog_reasoning: CatalogReasoningPolicy::OffOrMax,
        default_model: None,
    },
    ProviderDescriptor {
        id: ProviderId::OpenRouter,
        runtime: ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::OpenRouter,
            default_api_base: OPENROUTER_API_BASE,
        },
        name: "openrouter",
        display_name: "OpenRouter",
        auth_modes: &[
        AuthMode {
            id: "openrouter-api-key",
            login_label: "OpenRouter API key",
            auth_kind: ProviderAuthKind::ApiKey {
            env_var: "OPENROUTER_API_KEY",
            account: OPENROUTER_API_KEY_ACCOUNT,
            entry_label: "OpenRouter API key",
            missing_message: "missing OpenRouter API key; run /login openrouter in the TUI or set OPENROUTER_API_KEY as a CI/dev override",
        },
        },
        AuthMode {
            id: "openrouter-oauth",
            login_label: "OpenRouter OAuth",
            auth_kind: ProviderAuthKind::BearerCredential {
            env_var: "OPENROUTER_API_KEY",
            account: OPENROUTER_OAUTH_KEY_ACCOUNT,
            missing_message: "missing OpenRouter OAuth credentials; run /login openrouter-oauth in the TUI or set OPENROUTER_API_KEY as a CI/dev override",
            acquisition: BearerCredentialAcquisition::BrowserOAuth(BrowserOAuthFlow::OpenRouter),
        },
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "openrouter",
        catalog_reasoning: CatalogReasoningPolicy::OffAsNone,
        default_model: None,
    },
    ProviderDescriptor {
        id: ProviderId::KimiCode,
        runtime: ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::KimiCode,
            default_api_base: KIMI_CODE_API_BASE,
        },
        name: "kimi-code",
        display_name: "Kimi Code",
        auth_modes: &[
        AuthMode {
            id: "kimi-oauth",
            login_label: "Kimi Code OAuth",
            auth_kind: ProviderAuthKind::KimiOAuth {
            env_var: "KIMI_ACCESS_TOKEN",
            account: KIMI_TOKENS_ACCOUNT,
            missing_message: "missing Kimi OAuth credentials; run /login kimi-code or set KIMI_ACCESS_TOKEN as a CI/dev override",
        },
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "moonshotai",
        catalog_reasoning: CatalogReasoningPolicy::OffByAdvertisedToggle,
        default_model: None,
    },
    ProviderDescriptor {
        id: ProviderId::QwenTokenPlan,
        runtime: ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::QwenTokenPlan,
            default_api_base: QWEN_TOKEN_PLAN_API_BASE,
        },
        name: "qwen-token-plan",
        display_name: "Qwen Token Plan",
        auth_modes: &[
        AuthMode {
            id: "qwen-token-plan-api-key",
            login_label: "Qwen Token Plan API key",
            auth_kind: ProviderAuthKind::ApiKey {
            env_var: "QWEN_TOKEN_PLAN_API_KEY",
            account: QWEN_TOKEN_PLAN_API_KEY_ACCOUNT,
            entry_label: "Qwen Token Plan API key",
            missing_message: "missing Qwen Token Plan API key; run /login qwen-token-plan in the TUI or set QWEN_TOKEN_PLAN_API_KEY as a CI/dev override",
        },
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "alibaba-token-plan",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
        default_model: None,
    },
    ProviderDescriptor {
        id: ProviderId::Meta,
        runtime: ProviderRuntime::OpenAiCompatible {
            dialect: OpenAiCompatibleDialect::Standard,
            default_api_base: META_API_BASE,
        },
        name: "meta",
        display_name: "Meta Model API",
        auth_modes: &[
        AuthMode {
            id: "meta-api-key",
            login_label: "Meta Model API key",
            auth_kind: ProviderAuthKind::ApiKey {
            env_var: "MODEL_API_KEY",
            account: META_API_KEY_ACCOUNT,
            entry_label: "Meta Model API key",
            missing_message: "missing Meta Model API key; run /login meta in the TUI or set MODEL_API_KEY as a CI/dev override",
        },
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::OpenAiCompatible),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "meta",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
        default_model: Some("muse-spark-1.2"),
    },
    ProviderDescriptor {
        id: ProviderId::Xai,
        runtime: ProviderRuntime::Xai,
        name: "xai",
        display_name: "xAI",
        auth_modes: &[
        AuthMode {
            id: "xai-api-key",
            login_label: "xAI API key",
            auth_kind: ProviderAuthKind::ApiKey {
            env_var: "XAI_API_KEY",
            account: XAI_API_KEY_ACCOUNT,
            entry_label: "xAI API key",
            missing_message: "missing xAI API key; run /login xai in the TUI or set XAI_API_KEY as a CI/dev override",
        },
        },
        AuthMode {
            id: "xai-oauth",
            login_label: "xAI OAuth",
            auth_kind: ProviderAuthKind::XaiOAuth {
            env_var: "XAI_ACCESS_TOKEN",
            account: XAI_TOKENS_ACCOUNT,
            missing_message: "missing xAI OAuth credentials; run /login xai-oauth in the TUI or set XAI_ACCESS_TOKEN as a CI/dev override",
        },
        }
        ],
        model_source: ProviderModelSource::StaticCatalog,
        model_refresh: None,
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "xai",
        catalog_reasoning: CatalogReasoningPolicy::OffByAdvertisedToggle,
        default_model: None,
    },
    ProviderDescriptor {
        id: ProviderId::Cursor,
        runtime: ProviderRuntime::Cursor,
        name: "cursor",
        display_name: "Cursor",
        auth_modes: &[
        AuthMode {
            id: "cursor-oauth",
            login_label: "Cursor OAuth",
            auth_kind: ProviderAuthKind::CursorOAuth {
            env_var: "CURSOR_ACCESS_TOKEN",
            account: CURSOR_TOKENS_ACCOUNT,
            missing_message: "missing Cursor OAuth credentials; run /login cursor or set CURSOR_ACCESS_TOKEN as a CI/dev override",
        },
        }
        ],
        model_source: ProviderModelSource::CachedProviderModels,
        model_refresh: Some(ProviderModelRefreshKind::Cursor),
        model_id_codec: ModelIdCodec::Plain,
        metadata_upstream: "cursor",
        catalog_reasoning: CatalogReasoningPolicy::ExactAdvertised,
        default_model: Some("auto"),
    },
];
