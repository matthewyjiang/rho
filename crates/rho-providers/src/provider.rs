//! Stable provider identity, descriptor table, and runtime construction metadata.
//!
//! Credential-store access and HTTP adapters live elsewhere. Each
//! [`ProviderDescriptor`] owns its runtime shape and optional default model so
//! new providers are table rows rather than parallel match arms.

pub const OPENAI_API_KEY_ACCOUNT: &str = "provider:openai:api-key";
pub const ANTHROPIC_API_KEY_ACCOUNT: &str = "provider:anthropic:api-key";
pub const GOOGLE_API_KEY_ACCOUNT: &str = "provider:google:api-key";
pub const CODEX_TOKENS_ACCOUNT: &str = "provider:openai-codex:tokens";
pub const GITHUB_COPILOT_TOKENS_ACCOUNT: &str = "provider:github-copilot:tokens";
pub const XAI_API_KEY_ACCOUNT: &str = "provider:xai:api-key";
pub const XAI_TOKENS_ACCOUNT: &str = "provider:xai:tokens";
pub const MOONSHOT_API_KEY_ACCOUNT: &str = "provider:moonshot:api-key";
pub const OLLAMA_CLOUD_API_KEY_ACCOUNT: &str = "provider:ollama-cloud:api-key";
pub const POOLSIDE_API_KEY_ACCOUNT: &str = "provider:poolside:api-key";
pub const OPENROUTER_API_KEY_ACCOUNT: &str = "provider:openrouter:api-key";
pub const OPENROUTER_OAUTH_KEY_ACCOUNT: &str = "provider:openrouter:oauth-key";
pub const KIMI_TOKENS_ACCOUNT: &str = "provider:kimi-code:tokens";
pub const CURSOR_TOKENS_ACCOUNT: &str = "provider:cursor:tokens";
pub const QWEN_TOKEN_PLAN_API_KEY_ACCOUNT: &str = "provider:qwen-token-plan:api-key";
pub const META_API_KEY_ACCOUNT: &str = "provider:meta:api-key";

pub const OLLAMA_API_BASE: &str = "http://127.0.0.1:11434/v1";
pub const OLLAMA_CLOUD_API_BASE: &str = "https://ollama.com/v1";
pub const MOONSHOT_API_BASE: &str = "https://api.moonshot.ai/v1";
pub const POOLSIDE_API_BASE: &str = "https://inference.poolside.ai/v1";
pub const OPENROUTER_API_BASE: &str = "https://openrouter.ai/api/v1";
pub const KIMI_CODE_API_BASE: &str = "https://api.kimi.com/coding/v1";
pub const CURSOR_API_BASE: &str = "https://api2.cursor.sh";
/// Default OpenAI-compatible Token Plan base (Singapore / international).
pub const QWEN_TOKEN_PLAN_API_BASE: &str =
    "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1";
/// Meta Model API OpenAI-compatible base (Chat Completions and `/models`).
pub const META_API_BASE: &str = "https://api.meta.ai/v1";

/// OpenAI API-key vs Codex OAuth runtime auth selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAiRuntimeAuth {
    ApiKey,
    Codex,
}

/// How a registered provider is constructed at runtime.
///
/// Owned on [`ProviderDescriptor`] so adding a provider is a single table row
/// rather than a parallel match arm in the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderRuntime {
    OpenAi {
        auth_mode: OpenAiRuntimeAuth,
    },
    OpenAiCompatible {
        dialect: crate::openai_compatible_dialect::OpenAiCompatibleDialect,
        default_api_base: &'static str,
    },
    Anthropic,
    Google,
    GithubCopilot,
    Xai,
    Cursor,
}

impl ProviderRuntime {
    /// Whether two descriptors share a runtime family for auth-profile resolution.
    ///
    /// # Next major
    ///
    /// NEXT_MAJOR(rho-providers): remove ProviderRuntime::same_family.
    ///
    /// Kept only for compatibility; runtime equality conflates wire dialect and
    /// API base with auth family. New code should use
    /// [`same_provider_family`] with [`ProviderId`] so families are
    /// data-driven via `AUTH_FAMILY_GROUPS`.
    #[deprecated(note = "use provider::same_provider_family with ProviderId instead")]
    pub fn same_family(self, other: Self) -> bool {
        match (self, other) {
            (Self::OpenAi { .. }, Self::OpenAi { .. }) => true,
            (left, right) => left == right,
        }
    }
}

/// Provider families that share one backend for auth-profile switching.
///
/// OpenAI API-key and Codex OAuth are two auth modes on the same backend;
/// all other providers are isolated families. Add new groupings here rather
/// than hiding them in runtime equality.
///
/// Next-major intent: collapse `openai` + `codex` into one provider with two
/// auth modes (like `openrouter`/`xai` already do) so family grouping becomes
/// unnecessary; similarly `kimi-code` wants to be an auth mode on `moonshot`
/// rather than a separate provider with a different API base, which is why
/// `catalog::CROSS_PROVIDER_LOGIN_GROUPS` exists.
const AUTH_FAMILY_GROUPS: &[&[ProviderId]] = &[&[ProviderId::OpenAi, ProviderId::OpenAiCodex]];

pub fn same_provider_family(left: ProviderId, right: ProviderId) -> bool {
    if left == right {
        return true;
    }
    AUTH_FAMILY_GROUPS
        .iter()
        .any(|group| group.contains(&left) && group.contains(&right))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProviderId {
    Ollama,
    OllamaCloud,
    OpenAi,
    OpenAiCodex,
    Anthropic,
    Google,
    GithubCopilot,
    Xai,
    Moonshot,
    Poolside,
    OpenRouter,
    KimiCode,
    QwenTokenPlan,
    Meta,
    Cursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserOAuthFlow {
    OpenRouter,
}

impl BrowserOAuthFlow {
    pub const fn provider_label(self) -> &'static str {
        match self {
            Self::OpenRouter => "OpenRouter",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BearerCredentialAcquisition {
    BrowserOAuth(BrowserOAuthFlow),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderModelSource {
    StaticCatalog,
    CachedProviderModels,
}

/// Defines how raw models.dev reasoning controls become application capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogReasoningPolicy {
    /// Provider-specific protocol details cannot yet be represented safely.
    Unknown,
    /// This provider path does not forward a user reasoning control.
    NotConfigurable,
    /// Only controls explicitly advertised by the catalog are selectable.
    ExactAdvertised,
    /// A catalog toggle is a supported way to select `Off` for this protocol.
    OffByAdvertisedToggle,
    /// A reasoning model exposes a binary provider control as `Off` or `Max`.
    OffOrMax,
    /// The provider serializes `Off` as a provider-owned `none` control.
    OffAsNone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderModelRefreshKind {
    OpenAi,
    Anthropic,
    Google,
    GithubCopilot,
    OpenAiCompatible,
    Cursor,
}

/// How a provider encodes model IDs on the wire versus in Rho cache/config.
///
/// Discovery, selection, and request construction should use this policy instead
/// of hard-coding provider names at call sites.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ModelIdCodec {
    /// Cache, config, identity, and wire IDs all use the same model string.
    #[default]
    Plain,
    /// Wire IDs are `{provider_name}/{internal_id}`; cache and config store the
    /// internal id only. User-facing references remain `provider/internal_id`.
    ProviderPrefixed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderAuthKind {
    None,
    ApiKey {
        env_var: &'static str,
        account: &'static str,
        entry_label: &'static str,
        missing_message: &'static str,
    },
    CodexOAuth {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    GithubCopilotDevice {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    XaiOAuth {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    BearerCredential {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
        acquisition: BearerCredentialAcquisition,
    },
    KimiOAuth {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    CursorOAuth {
        env_var: &'static str,
        account: &'static str,
        missing_message: &'static str,
    },
    OllamaDeviceKey {
        missing_message: &'static str,
    },
}

impl ProviderDescriptor {
    /// Normalizes a model id for cache, config, and identity storage.
    ///
    /// For [`ModelIdCodec::ProviderPrefixed`], strips leading `{name}/` segments
    /// so legacy wire ids and double-prefixed favorites collapse to one internal id.
    pub fn canonicalize_model_id(&self, model: &str) -> String {
        match self.model_id_codec {
            ModelIdCodec::Plain => model.to_string(),
            ModelIdCodec::ProviderPrefixed => {
                let prefix = format!("{}/", self.name);
                let mut model = model;
                while let Some(rest) = model.strip_prefix(prefix.as_str()) {
                    if rest.is_empty() {
                        break;
                    }
                    model = rest;
                }
                model.to_string()
            }
        }
    }

    /// Expands an internal model id to the id sent on this provider's HTTP API.
    pub fn wire_model_id(&self, model: &str) -> String {
        match self.model_id_codec {
            ModelIdCodec::Plain => model.to_string(),
            ModelIdCodec::ProviderPrefixed => {
                let internal = self.canonicalize_model_id(model);
                format!("{}/{internal}", self.name)
            }
        }
    }

    /// Resolves a provider-facing model ID to its models.dev catalog ID.
    ///
    /// Provider model discovery remains authoritative. This only bridges model
    /// names when the provider API and metadata catalog use different IDs.
    pub fn metadata_model<'a>(&self, model: &'a str) -> &'a str {
        match (self.id, model) {
            (ProviderId::KimiCode, "k3") => "kimi-k3",
            (ProviderId::OpenRouter, model) => model
                .split_once('/')
                .map(|(_, upstream_model)| upstream_model)
                .unwrap_or(model),
            _ => model,
        }
    }

    /// Resolves an aggregator model ID to its models.dev provider.
    pub fn metadata_upstream_for_model<'a>(&self, model: &'a str) -> &'a str {
        match self.id {
            ProviderId::OpenRouter => model
                .split_once('/')
                .map(|(upstream, _)| upstream)
                .unwrap_or(self.metadata_upstream),
            _ => self.metadata_upstream,
        }
    }

    /// Returns a safe effective context when account-scoped model metadata is unavailable.
    pub fn effective_context_fallback(&self, model: &str) -> Option<u64> {
        match (self.id, model) {
            (ProviderId::KimiCode, "k3") => Some(262_144),
            _ => None,
        }
    }
}

impl ProviderAuthKind {
    pub fn env_var(self) -> Option<&'static str> {
        match self {
            Self::None | Self::OllamaDeviceKey { .. } => None,
            Self::ApiKey { env_var, .. }
            | Self::CodexOAuth { env_var, .. }
            | Self::GithubCopilotDevice { env_var, .. }
            | Self::XaiOAuth { env_var, .. }
            | Self::BearerCredential { env_var, .. }
            | Self::KimiOAuth { env_var, .. }
            | Self::CursorOAuth { env_var, .. } => Some(env_var),
        }
    }

    pub fn account(self) -> Option<&'static str> {
        match self {
            Self::None | Self::OllamaDeviceKey { .. } => None,
            Self::ApiKey { account, .. }
            | Self::CodexOAuth { account, .. }
            | Self::GithubCopilotDevice { account, .. }
            | Self::XaiOAuth { account, .. }
            | Self::BearerCredential { account, .. }
            | Self::KimiOAuth { account, .. }
            | Self::CursorOAuth { account, .. } => Some(account),
        }
    }

    /// User-facing guidance when this auth kind has no usable credentials.
    pub fn missing_message(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::ApiKey {
                missing_message, ..
            }
            | Self::CodexOAuth {
                missing_message, ..
            }
            | Self::GithubCopilotDevice {
                missing_message, ..
            }
            | Self::XaiOAuth {
                missing_message, ..
            }
            | Self::BearerCredential {
                missing_message, ..
            }
            | Self::KimiOAuth {
                missing_message, ..
            }
            | Self::CursorOAuth {
                missing_message, ..
            }
            | Self::OllamaDeviceKey {
                missing_message, ..
            } => Some(missing_message),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthMode {
    pub id: &'static str,
    pub login_label: &'static str,
    pub auth_kind: ProviderAuthKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    /// Runtime construction shape (dialect, default API base, backend kind).
    pub runtime: ProviderRuntime,
    pub name: &'static str,
    pub display_name: &'static str,
    /// Non-empty. First entry is the default auth mode.
    pub auth_modes: &'static [AuthMode],
    pub model_source: ProviderModelSource,
    pub model_refresh: Option<ProviderModelRefreshKind>,
    pub model_id_codec: ModelIdCodec,
    pub metadata_upstream: &'static str,
    pub catalog_reasoning: CatalogReasoningPolicy,
    /// Preferred model when the cache is empty or contains this id.
    pub default_model: Option<&'static str>,
}

/// Provider identity plus the selected auth mode after profile resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedProviderProfile {
    pub provider: &'static ProviderDescriptor,
    pub auth: AuthMode,
}

impl ResolvedProviderProfile {
    pub fn provider_name(self) -> &'static str {
        self.provider.name
    }

    pub fn auth_id(self) -> &'static str {
        self.auth.id
    }

    pub fn auth_kind(self) -> ProviderAuthKind {
        self.auth.auth_kind
    }
}

impl ProviderDescriptor {
    /// First registered auth mode; every descriptor has at least one.
    pub fn default_auth(self) -> AuthMode {
        self.auth_modes[0]
    }

    /// Iterates registered auth modes in declaration order.
    pub fn auth_modes(self) -> impl Iterator<Item = AuthMode> {
        self.auth_modes.iter().copied()
    }

    pub fn auth_mode(self, auth: &str) -> Option<AuthMode> {
        self.auth_modes().find(|mode| mode.id == auth)
    }

    pub fn is_keyless(self) -> bool {
        matches!(self.default_auth().auth_kind, ProviderAuthKind::None)
    }
}

#[path = "provider_table.rs"]
mod provider_table;

pub use provider_table::PROVIDERS;

pub fn providers() -> &'static [ProviderDescriptor] {
    PROVIDERS
}

/// Environment variable names used as provider credential overrides.
///
/// Derived from [`PROVIDERS`] auth kinds so newly registered provider credentials
/// are included automatically. Hosts should strip these from child process
/// environments by default, for example with
/// [`rho_sdk::ProcessEnvironment::inherit_except`].
pub fn credential_env_vars() -> &'static [&'static str] {
    use std::sync::OnceLock;

    static VARS: OnceLock<Vec<&'static str>> = OnceLock::new();
    VARS.get_or_init(|| {
        let mut vars: Vec<&'static str> = PROVIDERS
            .iter()
            .flat_map(|descriptor| descriptor.auth_modes())
            .filter_map(|mode| mode.auth_kind.env_var())
            .collect();
        vars.sort_unstable();
        vars.dedup();
        vars
    })
    .as_slice()
}

/// Auth profile names accepted by CLI `--auth` and config `auth`.
///
/// Derived from [`PROVIDERS`] so newly registered profiles are included
/// automatically. Keyless providers use `auth = "none"` and are omitted.
pub fn auth_profiles() -> &'static [&'static str] {
    use std::sync::OnceLock;

    static PROFILES: OnceLock<Vec<&'static str>> = OnceLock::new();
    PROFILES
        .get_or_init(|| {
            PROVIDERS
                .iter()
                .flat_map(|descriptor| descriptor.auth_modes())
                .map(|mode| mode.id)
                .filter(|auth| *auth != "none")
                .collect()
        })
        .as_slice()
}

/// Maps a retired same-API provider name to its provider and auth mode.
///
/// The auth mode is fixed so legacy config such as `provider = "xai-oauth"`
/// keeps its old meaning even when paired with a stale or default `auth` value.
pub fn legacy_provider_alias(provider: &str) -> Option<(&'static str, &'static str)> {
    match provider {
        "openrouter-oauth" => Some(("openrouter", "openrouter-oauth")),
        "xai-oauth" => Some(("xai", "xai-oauth")),
        _ => None,
    }
}

/// Looks up a canonical provider name.
///
/// Legacy provider references carry an auth-mode choice, so callers handling
/// external input must use [`resolve_provider_reference`] or [`resolve_profile`]
/// rather than discarding that choice here.
pub fn provider_descriptor(provider: &str) -> Option<&'static ProviderDescriptor> {
    providers()
        .iter()
        .find(|descriptor| descriptor.name == provider)
}

/// Formats a provider-qualified model reference for user input and display.
pub fn model_reference(provider: &str, model: &str) -> String {
    format!("{provider}/{model}")
}

pub fn provider_descriptor_for_auth(auth: &str) -> Option<&'static ProviderDescriptor> {
    providers()
        .iter()
        .find(|descriptor| descriptor.auth_mode(auth).is_some())
}

/// Resolves an auth profile id to its provider and mode.
pub fn resolve_auth_mode(auth: &str) -> Option<(&'static ProviderDescriptor, AuthMode)> {
    let descriptor = provider_descriptor_for_auth(auth)?;
    let mode = descriptor.auth_mode(auth)?;
    Some((descriptor, mode))
}

fn legacy_provider_profile(provider_name: &str) -> Option<ResolvedProviderProfile> {
    let (provider, auth) = legacy_provider_alias(provider_name)?;
    let provider = provider_descriptor(provider).expect("alias provider must be registered");
    let auth = provider
        .auth_mode(auth)
        .expect("alias auth mode must be registered on its provider");
    Some(ResolvedProviderProfile { provider, auth })
}

/// Resolves a provider reference without a separately supplied auth mode.
///
/// Canonical names select their default auth mode. Retired provider names keep
/// selecting the auth mode that was implicit in the old provider identity.
pub fn resolve_provider_reference(
    provider_name: &str,
) -> Result<ResolvedProviderProfile, ProfileResolutionError> {
    if let Some(profile) = legacy_provider_profile(provider_name) {
        return Ok(profile);
    }
    let provider = provider_descriptor(provider_name)
        .ok_or_else(|| ProfileResolutionError::UnknownProvider(provider_name.into()))?;
    Ok(ResolvedProviderProfile {
        provider,
        auth: provider.default_auth(),
    })
}

/// Resolves a provider/auth pair to one registered provider identity and auth mode.
///
/// Soft resolution: when the requested auth belongs to a different provider
/// family, falls back to the named provider's default auth mode. Prefer
/// [`resolve_profile_exact`] when a caller must reject incompatible pairs
/// (agent pins, catalog validation).
pub fn resolve_profile(
    provider_name: &str,
    auth: &str,
) -> Result<ResolvedProviderProfile, ProfileResolutionError> {
    match resolve_profile_exact(provider_name, auth) {
        Err(ProfileResolutionError::AuthNotValidForProvider { provider, .. }) => {
            let provider = provider_descriptor(&provider)
                .ok_or(ProfileResolutionError::UnknownProvider(provider))?;
            Ok(ResolvedProviderProfile {
                provider,
                auth: provider.default_auth(),
            })
        }
        other => other,
    }
}

/// Resolves a provider/auth pair only when the auth mode belongs to that provider.
///
/// Unlike [`resolve_profile`], incompatible pairs error instead of falling back
/// to the provider default. Legacy provider aliases still resolve to their
/// fixed auth mode (the alias is the identity).
pub fn resolve_profile_exact(
    provider_name: &str,
    auth: &str,
) -> Result<ResolvedProviderProfile, ProfileResolutionError> {
    if let Some(profile) = legacy_provider_profile(provider_name) {
        return Ok(profile);
    }
    let provider = provider_descriptor(provider_name)
        .ok_or_else(|| ProfileResolutionError::UnknownProvider(provider_name.into()))?;
    if let Some(mode) = provider.auth_mode(auth) {
        return Ok(ResolvedProviderProfile {
            provider,
            auth: mode,
        });
    }
    let auth_profile = provider_descriptor_for_auth(auth)
        .ok_or_else(|| ProfileResolutionError::UnknownAuth(auth.into()))?;
    if same_provider_family(provider.id, auth_profile.id) {
        let mode = auth_profile
            .auth_mode(auth)
            .expect("auth exists on auth_profile");
        Ok(ResolvedProviderProfile {
            provider: auth_profile,
            auth: mode,
        })
    } else {
        Err(ProfileResolutionError::AuthNotValidForProvider {
            provider: provider_name.into(),
            auth: auth.into(),
        })
    }
}

/// Returns whether `auth` is a registered mode for `provider_name`.
///
/// Uses exact membership ([`resolve_profile_exact`]), not soft default fallback.
pub fn provider_accepts_auth(provider_name: &str, auth: &str) -> bool {
    resolve_profile_exact(provider_name, auth).is_ok()
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProfileResolutionError {
    #[error("unknown provider '{0}'")]
    UnknownProvider(String),
    #[error("unknown auth profile '{0}'")]
    UnknownAuth(String),
    #[error("auth '{auth}' is not valid for provider '{provider}'")]
    AuthNotValidForProvider { provider: String, auth: String },
}

pub fn provider_descriptor_by_id(id: ProviderId) -> &'static ProviderDescriptor {
    providers()
        .iter()
        .find(|descriptor| descriptor.id == id)
        .expect("every provider ID must have a descriptor")
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
