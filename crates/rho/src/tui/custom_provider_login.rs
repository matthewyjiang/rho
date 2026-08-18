//! `/login` onboarding for a user-defined OpenAI-compatible host.

use rho_providers::{
    model::catalog::LoginTarget,
    provider::{self, custom_provider_api_key_auth_id},
};

use super::{login::SecretInput, text_input::TextInput, App, ComposerMode, Entry};

pub(super) const CUSTOM_PROVIDER_LOGIN_VALUE: &str = "_custom-chat-completions";
pub(super) const CUSTOM_PROVIDER_LOGIN_LABEL: &str = "Custom Chat Completions";
pub(super) const CUSTOM_PROVIDER_LOGIN_DETAIL: &str =
    "Name a Chat Completions host, set its URL, and optionally store an API key.";

impl App {
    pub(super) fn start_custom_provider_onboarding(&mut self) {
        self.input_ui
            .set_composer(ComposerMode::TextInput(TextInput::custom_provider_name()));
        self.set_status("name the custom provider");
    }

    pub(super) fn submit_custom_provider_name(&mut self, name: String) -> anyhow::Result<bool> {
        let name = name.trim().to_ascii_lowercase();
        if let Err(error) = provider::validate_custom_provider_name(&name) {
            self.insert_entry(&Entry::Error(error.to_string()));
            self.set_status("login failed");
            self.input_ui
                .set_composer(ComposerMode::TextInput(TextInput::custom_provider_name()));
            return Ok(true);
        }
        self.input_ui
            .set_composer(ComposerMode::TextInput(TextInput::custom_provider_url(
                name,
            )));
        self.set_status("enter the custom provider URL");
        Ok(true)
    }

    pub(super) fn submit_custom_provider_url(
        &mut self,
        name: String,
        base_url: String,
    ) -> anyhow::Result<bool> {
        let base_url = base_url.trim().to_string();
        if let Err(error) = self.persist_custom_provider(&name, &base_url) {
            self.insert_entry(&Entry::Error(error.to_string()));
            self.set_status("login failed");
            self.input_ui
                .set_composer(ComposerMode::TextInput(TextInput::custom_provider_url(
                    name,
                )));
            return Ok(true);
        }
        self.insert_entry(&Entry::Notice(format!(
            "saved custom provider {name} at {base_url}"
        )));
        let target = custom_provider_login_target(&name);
        self.input_ui
            .set_composer(ComposerMode::SecretInput(SecretInput::optional(target)));
        self.set_status("enter API key or leave blank");
        Ok(true)
    }

    fn persist_custom_provider(&mut self, name: &str, base_url: &str) -> anyhow::Result<()> {
        self.info
            .services
            .config_repository
            .update(|config| config.providers.set_endpoint(name, base_url))??;
        let config = self.info.services.config_repository.load()?;
        config.providers.activate()?;
        config.providers.refresh_thread_visibility()?;
        Ok(())
    }
}

fn custom_provider_login_target(name: &str) -> LoginTarget {
    LoginTarget {
        provider: name.to_string(),
        auth: custom_provider_api_key_auth_id(name),
        label: format!("{name} API key"),
    }
}

pub(super) fn is_custom_provider_login_value(value: &str) -> bool {
    value == CUSTOM_PROVIDER_LOGIN_VALUE
}
