//! `/login` onboarding for OpenAI-compatible hosts that persist an API base.
//!
//! Custom hosts pick Chat Completions or Responses from the provider picker,
//! then collect a name, a URL, and an optional key. Built-in hosts that
//! [`provider::ProviderDescriptor::collects_login_endpoint`] start on the URL
//! step. The name and URL steps reuse the shared [`TextInput`] overlay and
//! carry their own state in [`CustomHostStep`], so that widget stays a plain
//! line editor.

use rho_providers::{model::catalog, provider, provider::OpenAiCompatibleApi};

use super::{login::SecretInput, text_input::TextInput, App, ComposerMode, Entry, PickerItem};

/// Picker value for a new Chat Completions host.
///
/// Underscore-prefixed so it cannot collide with a validated host name, which
/// must start with a lowercase letter.
pub(super) const NEW_CUSTOM_CHAT_COMPLETIONS_HOST_VALUE: &str = "_custom-chat-completions";
/// Picker value for a new Responses host.
pub(super) const NEW_CUSTOM_RESPONSES_HOST_VALUE: &str = "_custom-responses";

const CUSTOM_CHAT_COMPLETIONS_LOGIN_LABEL: &str = "Custom · Chat Completions";
const CUSTOM_RESPONSES_LOGIN_LABEL: &str = "Custom · Responses";
const CUSTOM_CHAT_COMPLETIONS_LOGIN_DETAIL: &str =
    "POST {base}/chat/completions. Name the host, set its URL, and optionally store an API key.";
const CUSTOM_RESPONSES_LOGIN_DETAIL: &str =
    "POST {base}/responses. Name the host, set its URL, and optionally store an API key.";

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8000/v1";

/// Which field of the wizard the shared text overlay is currently editing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CustomHostStep {
    /// Provider name, carrying the wire API chosen in the login picker.
    Name { api: OpenAiCompatibleApi },
    /// Base URL for a new custom host, carrying the same wire API.
    CustomUrl {
        name: String,
        api: OpenAiCompatibleApi,
    },
    /// Base URL for a built-in that only persists an endpoint.
    BuiltinUrl { name: String },
}

impl CustomHostStep {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Name { .. } => "provider name",
            Self::CustomUrl { .. } | Self::BuiltinUrl { .. } => "base URL",
        }
    }
}

/// Wire API encoded in a custom-host picker value, if this is one.
pub(super) fn parse_custom_host_api(value: &str) -> Option<OpenAiCompatibleApi> {
    match value {
        NEW_CUSTOM_CHAT_COMPLETIONS_HOST_VALUE => Some(OpenAiCompatibleApi::ChatCompletions),
        NEW_CUSTOM_RESPONSES_HOST_VALUE => Some(OpenAiCompatibleApi::Responses),
        _ => None,
    }
}

/// Top-level `/login` rows for creating a host that does not exist yet.
pub(super) fn login_group_items() -> [PickerItem; 2] {
    [
        PickerItem {
            section: None,
            label: CUSTOM_CHAT_COMPLETIONS_LOGIN_LABEL.into(),
            detail: Some(CUSTOM_CHAT_COMPLETIONS_LOGIN_DETAIL.into()),
            preview: None,
            badge: None,
            value: NEW_CUSTOM_CHAT_COMPLETIONS_HOST_VALUE.into(),
            selection_verb: None,
        },
        PickerItem {
            section: None,
            label: CUSTOM_RESPONSES_LOGIN_LABEL.into(),
            detail: Some(CUSTOM_RESPONSES_LOGIN_DETAIL.into()),
            preview: None,
            badge: None,
            value: NEW_CUSTOM_RESPONSES_HOST_VALUE.into(),
            selection_verb: None,
        },
    ]
}

impl App {
    pub(super) fn start_custom_provider_onboarding(&mut self, api: OpenAiCompatibleApi) {
        self.edit_custom_host_step(
            CustomHostStep::Name { api },
            String::new(),
            "name the custom provider",
        );
    }

    /// Opens the URL step for a built-in that persists its API base from `/login`.
    pub(super) fn start_endpoint_onboarding(&mut self, provider: &str) {
        match self.endpoint_prefill(provider) {
            Ok(prefill) => self.edit_custom_host_step(
                CustomHostStep::BuiltinUrl {
                    name: provider.to_string(),
                },
                prefill,
                "enter the API base URL",
            ),
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not load config before login: {err}"
                )));
                self.set_status("login failed");
            }
        }
    }

    fn endpoint_prefill(&self, provider: &str) -> anyhow::Result<String> {
        let config = self.info.services.config_repository.load()?;
        Ok(config
            .resolved_provider_endpoint(provider)
            .map(|url| url.to_string())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()))
    }

    /// Opens one wizard step, seeded with `value` so a rejected entry survives.
    fn edit_custom_host_step(&mut self, step: CustomHostStep, value: String, status: &'static str) {
        self.input_ui
            .set_composer(ComposerMode::TextInput(TextInput::custom_host(step, value)));
        self.set_status(status);
    }

    pub(super) fn submit_custom_host_step(
        &mut self,
        step: CustomHostStep,
        value: String,
    ) -> anyhow::Result<()> {
        match step {
            CustomHostStep::Name { api } => self.submit_custom_provider_name(api, value),
            CustomHostStep::CustomUrl { name, api } => {
                self.submit_custom_provider_url(name, api, value)
            }
            CustomHostStep::BuiltinUrl { name } => self.submit_builtin_endpoint_url(name, value),
        }
    }

    fn submit_custom_provider_name(
        &mut self,
        api: OpenAiCompatibleApi,
        value: String,
    ) -> anyhow::Result<()> {
        let name = value.trim().to_ascii_lowercase();
        if let Err(error) = provider::validate_custom_provider_name(&name) {
            return self.retry_custom_host_step(CustomHostStep::Name { api }, value, error);
        }
        self.edit_custom_host_step(
            CustomHostStep::CustomUrl { name, api },
            DEFAULT_BASE_URL.into(),
            "enter the custom provider URL",
        );
        Ok(())
    }

    fn submit_custom_provider_url(
        &mut self,
        name: String,
        api: OpenAiCompatibleApi,
        value: String,
    ) -> anyhow::Result<()> {
        let base_url = value.trim().to_string();
        if let Err(error) = self.persist_custom_provider(&name, &base_url, api) {
            return self.retry_custom_host_step(
                CustomHostStep::CustomUrl { name, api },
                value,
                error,
            );
        }
        self.after_endpoint_saved(&name, &base_url)
    }

    fn submit_builtin_endpoint_url(&mut self, name: String, value: String) -> anyhow::Result<()> {
        let base_url = value.trim().to_string();
        if let Err(error) = self.persist_builtin_endpoint(&name, &base_url) {
            return self.retry_custom_host_step(CustomHostStep::BuiltinUrl { name }, value, error);
        }
        self.after_endpoint_saved(&name, &base_url)
    }

    fn after_endpoint_saved(&mut self, name: &str, base_url: &str) -> anyhow::Result<()> {
        self.insert_entry(&Entry::Notice(saved_endpoint_notice(name, base_url)));
        // The host is interned now, so its API-key target comes from the registry.
        let Some(target) = catalog::login_target_for_provider(name) else {
            self.insert_entry(&Entry::Error(format!(
                "saved custom provider {name}, but it has no API key login"
            )));
            self.set_status("login failed");
            return Ok(());
        };
        self.input_ui
            .set_composer(ComposerMode::SecretInput(SecretInput::optional(target)));
        self.set_status("enter API key or leave blank");
        Ok(())
    }

    /// Reports the failure and reopens the same step with what the user typed.
    fn retry_custom_host_step(
        &mut self,
        step: CustomHostStep,
        value: String,
        error: anyhow::Error,
    ) -> anyhow::Result<()> {
        self.insert_entry(&Entry::Error(error.to_string()));
        self.edit_custom_host_step(step, value, "login failed");
        Ok(())
    }

    pub(super) fn cancel_custom_host_step(&mut self) {
        self.input_ui.set_composer(ComposerMode::Input);
        self.set_status("login cancelled");
    }

    fn persist_custom_provider(
        &mut self,
        name: &str,
        base_url: &str,
        api: OpenAiCompatibleApi,
    ) -> anyhow::Result<()> {
        self.info.services.config_repository.update(|config| {
            config.providers.set_endpoint(name, base_url)?;
            config.providers.set_openai_compatible_api(name, api)?;
            // Nothing overlays the process-wide set, so installing is enough
            // to make the host visible here and in every spawned task.
            config.providers.activate()
        })?
    }

    fn persist_builtin_endpoint(&mut self, name: &str, base_url: &str) -> anyhow::Result<()> {
        self.info.services.config_repository.update(|config| {
            config.providers.set_endpoint(name, base_url)?;
            config.providers.activate()
        })?
    }
}

fn saved_endpoint_notice(name: &str, base_url: &str) -> String {
    match provider::provider_descriptor(name) {
        Some(descriptor) if descriptor.collects_login_endpoint() => {
            format!("saved {} endpoint {base_url}", descriptor.display_name)
        }
        _ => format!("saved custom provider {name} at {base_url}"),
    }
}
