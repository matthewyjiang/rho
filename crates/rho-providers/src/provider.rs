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
pub const QWEN_TOKEN_PLAN_API_KEY_ACCOUNT: &str = "provider:qwen-token-plan:api-key";
pub const META_API_KEY_ACCOUNT: &str = "provider:meta:api-key";
pub const OPENCODE_GO_API_KEY_ACCOUNT: &str = "provider:opencode-go:api-key";

/// Auth profile id meaning "this provider needs no credential".
pub const KEYLESS_AUTH: &str = "none";

pub const OLLAMA_API_BASE: &str = "http://127.0.0.1:11434/v1";
pub const OLLAMA_CLOUD_API_BASE: &str = "https://ollama.com/v1";
pub const MOONSHOT_API_BASE: &str = "https://api.moonshot.ai/v1";
pub const POOLSIDE_API_BASE: &str = "https://inference.poolside.ai/v1";
pub const OPENROUTER_API_BASE: &str = "https://openrouter.ai/api/v1";
pub const KIMI_CODE_API_BASE: &str = "https://api.kimi.com/coding/v1";
/// Default OpenAI-compatible Token Plan base (Singapore / international).
pub const QWEN_TOKEN_PLAN_API_BASE: &str =
    "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1";
/// Meta Model API OpenAI-compatible base (Chat Completions and `/models`).
pub const META_API_BASE: &str = "https://api.meta.ai/v1";
/// OpenCode Go bootstrap base (Chat Completions, Responses, Messages, `/models`).
pub const OPENCODE_GO_API_BASE: &str = "https://opencode.ai/zen/go/v1";
/// Placeholder only. Config-defined hosts must take their API base from application config.
pub const OPENAI_COMPATIBLE_API_BASE: &str = "http://127.0.0.1:0/v1";

/// Ollama's OpenAI-compatible API accepts only these effort values.
pub const OLLAMA_UNKNOWN_REASONING_LEVELS: &[crate::reasoning::ReasoningLevel] = &[
    crate::reasoning::ReasoningLevel::Off,
    crate::reasoning::ReasoningLevel::Low,
    crate::reasoning::ReasoningLevel::Medium,
    crate::reasoning::ReasoningLevel::High,
    crate::reasoning::ReasoningLevel::Max,
];

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
        /// Whether construction may follow the catalog's models.dev `npm`
        /// mapping instead of the declared Chat Completions runtime.
        catalog_construction: CatalogConstruction,
    },
    Anthropic,
    Google,
    GithubCopilot,
    Xai,
}

impl ProviderRuntime {
    /// Only Chat Completions runtimes can defer their wire adapter to the
    /// catalog; every other runtime constructs as declared.
    pub fn catalog_construction(self) -> CatalogConstruction {
        match self {
            Self::OpenAiCompatible {
                catalog_construction,
                ..
            } => catalog_construction,
            Self::OpenAi { .. }
            | Self::Anthropic
            | Self::Google
            | Self::GithubCopilot
            | Self::Xai => CatalogConstruction::Runtime,
        }
    }

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

/// # Next major
///
/// NEXT_MAJOR(rho-providers): add a variant for config-defined OpenAI-compatible
/// hosts so their identity is not aliased onto a built-in id.
///
/// Until then, named custom hosts reuse [`ProviderId::Ollama`] as a wire-family
/// stand-in. Callers must use [`ProviderDescriptor::name`] and
/// [`ProviderDescriptor::is_custom_openai_compatible`] for identity.
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
    OpenCodeGo,
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

/// How an OpenAI-compatible table row chooses its HTTP adapter.
///
/// [`Self::Runtime`] uses [`ProviderRuntime`] only. [`Self::PreferModelsDevNpm`]
/// may construct Responses or Anthropic adapters when models.dev names a
/// different AI SDK package for the selected model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CatalogConstruction {
    #[default]
    Runtime,
    PreferModelsDevNpm,
}

/// What to serialize when a Standard-dialect model is missing catalog metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownEffortPolicy {
    /// Omit the field so the host applies its own default.
    Omit,
    /// Send the requested level, including `Off` as `"none"`.
    SendRequested,
    /// Map the request onto this fixed vocabulary.
    Constrain(&'static [crate::reasoning::ReasoningLevel]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderModelRefreshKind {
    OpenAi,
    Anthropic,
    Google,
    GithubCopilot,
    OpenAiCompatible,
    /// Native Ollama `/api/tags` + `/api/show`, falling back to `/v1/models`.
    Ollama,
}

impl ProviderModelRefreshKind {
    /// True when `/doctor` and health probes hit `/v1/models`.
    pub(crate) fn probes_openai_compatible_models(self) -> bool {
        matches!(self, Self::OpenAiCompatible | Self::Ollama)
    }
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
            | Self::KimiOAuth { env_var, .. } => Some(env_var),
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
            | Self::KimiOAuth { account, .. } => Some(account),
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

    /// True when every registered mode is keyless.
    pub fn is_keyless(self) -> bool {
        self.auth_modes()
            .all(|mode| matches!(mode.auth_kind, ProviderAuthKind::None))
    }

    /// True when the provider can run without credentials.
    pub fn has_none_auth(self) -> bool {
        self.auth_modes()
            .any(|mode| matches!(mode.auth_kind, ProviderAuthKind::None))
    }

    /// Config-defined Chat Completions hosts are named providers, not a single built-in.
    pub fn is_custom_openai_compatible(self) -> bool {
        PROVIDERS.iter().all(|builtin| builtin.name != self.name)
    }

    /// Whether `/doctor` and `/config` can reach this host's `/v1/models`.
    ///
    /// A configured endpoint plus OpenAI-compatible discovery is enough; the
    /// probe supplies whatever credentials the host has, so a stored key does
    /// not disqualify it.
    pub fn probes_configured_endpoint(self) -> bool {
        self.has_none_auth()
            && self
                .model_refresh
                .is_some_and(ProviderModelRefreshKind::probes_openai_compatible_models)
    }

    /// Auth mode used for unattended model discovery.
    ///
    /// Prefers a mode whose credentials are present so a keyed custom host is
    /// probed with its key instead of anonymously; falls back to the default.
    pub fn discovery_auth(self, store: &dyn crate::credentials::CredentialStore) -> AuthMode {
        self.auth_modes()
            .find(|mode| {
                !matches!(mode.auth_kind, ProviderAuthKind::None)
                    && crate::credentials::auth_has_credentials(store, mode.id).unwrap_or(false)
            })
            .unwrap_or_else(|| self.default_auth())
    }

    /// Wire policy for Standard-dialect hosts when models.dev has no row.
    pub fn unknown_effort(self) -> UnknownEffortPolicy {
        if self.is_custom_openai_compatible() {
            return UnknownEffortPolicy::SendRequested;
        }
        match self.id {
            ProviderId::Ollama | ProviderId::OllamaCloud => {
                UnknownEffortPolicy::Constrain(OLLAMA_UNKNOWN_REASONING_LEVELS)
            }
            _ => UnknownEffortPolicy::Omit,
        }
    }
}

#[path = "provider_table.rs"]
mod provider_table;

#[path = "custom_openai_compatible.rs"]
mod custom_openai_compatible;

pub(crate) use custom_openai_compatible::interned_custom_providers;
pub use custom_openai_compatible::{
    custom_provider_api_key_auth_id, custom_provider_registry_test_lock,
    install_custom_openai_compatible_providers, intern_custom_openai_compatible_providers,
    interned_custom_provider, is_custom_provider_api_key_auth,
    reset_custom_openai_compatible_providers_for_tests, scope_custom_openai_compatible_providers,
    validate_custom_provider_name, CustomProviderSpec, CustomProviderThreadScope,
};
pub use provider_table::PROVIDERS;

pub fn providers() -> &'static [ProviderDescriptor] {
    PROVIDERS
}

/// Built-in providers plus the currently visible custom OpenAI-compatible hosts.
///
/// # Next major
///
/// NEXT_MAJOR(rho-providers): make [`providers`] include config-defined hosts,
/// or replace both with one iterator, so callers do not choose between the
/// static table and a visibility snapshot.
pub fn visible_providers() -> Vec<&'static ProviderDescriptor> {
    let mut providers = PROVIDERS.iter().collect::<Vec<_>>();
    providers.extend(custom_openai_compatible::custom_openai_compatible_providers());
    providers
}

/// Environment variable names used as provider credential overrides.
///
/// Built-in auth kinds, interned custom hosts, and any currently set
/// `RHO_*_API_KEY`. Callers typically snapshot this into
/// [`rho_sdk::ProcessEnvironment::inherit_except`] at tool-set construction;
/// scanning the live environment covers an override for a host that `/login`
/// interns later in the same session.
///
/// Intern config-defined hosts before calling this: a host that has never been
/// interned has no descriptor and therefore no override name to report, unless
/// its `RHO_*_API_KEY` is already set.
pub fn credential_env_vars() -> Vec<String> {
    credential_env_vars_from(std::env::vars().map(|(name, _)| name))
}

pub(crate) fn credential_env_vars_from(
    env: impl IntoIterator<Item = impl AsRef<str>>,
) -> Vec<String> {
    let mut vars: Vec<String> = providers()
        .iter()
        .chain(custom_openai_compatible::interned_custom_providers())
        .flat_map(|descriptor| descriptor.auth_modes())
        .filter_map(|mode| mode.auth_kind.env_var())
        .map(str::to_owned)
        .collect();
    vars.extend(env.into_iter().filter_map(|name| {
        let name = name.as_ref();
        custom_openai_compatible::is_provider_api_key_env_var(name).then(|| name.to_owned())
    }));
    vars.sort_unstable();
    vars.dedup();
    vars
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
    PROVIDERS
        .iter()
        .find(|descriptor| descriptor.name == provider)
        .or_else(|| custom_openai_compatible::custom_openai_compatible_provider(provider))
}

pub fn interned_custom_openai_compatible_provider(
    provider: &str,
) -> Option<&'static ProviderDescriptor> {
    custom_openai_compatible::interned_custom_provider(provider)
}

/// Formats a provider-qualified model reference for user input and display.
pub fn model_reference(provider: &str, model: &str) -> String {
    format!("{provider}/{model}")
}

pub fn provider_descriptor_for_auth(auth: &str) -> Option<&'static ProviderDescriptor> {
    providers()
        .iter()
        .find(|descriptor| descriptor.auth_mode(auth).is_some())
        .or_else(|| custom_openai_compatible::interned_custom_provider_for_auth(auth))
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
    if !provider.is_custom_openai_compatible() && same_provider_family(provider.id, auth_profile.id)
    {
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
        .expect("every built-in provider ID must have a descriptor")
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
