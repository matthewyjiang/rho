//! `/login` onboarding for a user-defined OpenAI-compatible host.
//!
//! Three steps: name the host, set its base URL, then optionally store an API
//! key. The first two reuse the shared [`TextInput`] overlay and carry their own
//! state in [`CustomHostStep`], so that widget stays a plain line editor.

use rho_providers::{model::catalog, provider};

use super::{login::SecretInput, text_input::TextInput, App, ComposerMode, Entry};

/// Picker value for "create a host that does not exist yet".
///
/// Underscore-prefixed so it cannot collide with a validated host name, which
/// must start with a lowercase letter.
pub(super) const NEW_CUSTOM_HOST_VALUE: &str = "_custom-chat-completions";
pub(super) const CUSTOM_PROVIDER_LOGIN_LABEL: &str = "Custom Chat Completions";
pub(super) const CUSTOM_PROVIDER_LOGIN_DETAIL: &str =
    "Name a Chat Completions host, set its URL, and optionally store an API key.";

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8000/v1";

/// Which field of the wizard the shared text overlay is currently editing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CustomHostStep {
    Name,
    /// Base URL for the host named in the previous step.
    Url {
        name: String,
    },
}

impl CustomHostStep {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Name => "provider name",
            Self::Url { .. } => "base URL",
        }
    }
}

impl App {
    pub(super) fn start_custom_provider_onboarding(&mut self) {
        self.edit_custom_host_step(
            CustomHostStep::Name,
            String::new(),
            "name the custom provider",
        );
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
            CustomHostStep::Name => self.submit_custom_provider_name(value),
            CustomHostStep::Url { name } => self.submit_custom_provider_url(name, value),
        }
    }

    fn submit_custom_provider_name(&mut self, value: String) -> anyhow::Result<()> {
        let name = value.trim().to_ascii_lowercase();
        if let Err(error) = provider::validate_custom_provider_name(&name) {
            return self.retry_custom_host_step(CustomHostStep::Name, value, error);
        }
        self.edit_custom_host_step(
            CustomHostStep::Url { name },
            DEFAULT_BASE_URL.into(),
            "enter the custom provider URL",
        );
        Ok(())
    }

    fn submit_custom_provider_url(&mut self, name: String, value: String) -> anyhow::Result<()> {
        let base_url = value.trim().to_string();
        if let Err(error) = self.persist_custom_provider(&name, &base_url) {
            return self.retry_custom_host_step(CustomHostStep::Url { name }, value, error);
        }
        self.insert_entry(&Entry::Notice(format!(
            "saved custom provider {name} at {base_url}"
        )));
        // The host is interned now, so its API-key target comes from the registry.
        let Some(target) = catalog::login_target_for_provider(&name) else {
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

    fn persist_custom_provider(&mut self, name: &str, base_url: &str) -> anyhow::Result<()> {
        self.info.services.config_repository.update(|config| {
            config.providers.set_endpoint(name, base_url)?;
            // Nothing overlays the process-wide set, so installing is enough
            // to make the host visible here and in every spawned task.
            config.providers.activate()
        })?
    }
}
