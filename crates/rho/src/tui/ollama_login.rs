//! `/login` onboarding for the built-in Ollama host.
//!
//! First-run config does not invent a default endpoint. `/login ollama` asks
//! for the API base (prefilled with the built-in default) and then an optional
//! API key, matching custom Chat Completions hosts.

use rho_providers::model::catalog;

use super::{login::SecretInput, text_input::TextInput, App, ComposerMode, Entry};
use crate::config::DEFAULT_OLLAMA_BASE_URL;

impl App {
    pub(super) fn start_ollama_onboarding(&mut self) {
        let prefill = self
            .info
            .services
            .config_repository
            .configured_path()
            .and_then(crate::config::Config::load_settings_only)
            .ok()
            .and_then(|config| {
                config
                    .providers
                    .ollama
                    .map(|endpoint| endpoint.base_url.to_string())
            })
            .unwrap_or_else(|| DEFAULT_OLLAMA_BASE_URL.to_string());
        self.edit_ollama_endpoint(prefill, "enter the Ollama API base URL");
    }

    fn edit_ollama_endpoint(&mut self, value: String, status: &'static str) {
        self.input_ui
            .set_composer(ComposerMode::TextInput(TextInput::ollama_endpoint(value)));
        self.set_status(status);
    }

    pub(super) fn submit_ollama_endpoint(&mut self, value: String) -> anyhow::Result<()> {
        let base_url = value.trim().to_string();
        if let Err(error) = self.persist_ollama_endpoint(&base_url) {
            self.insert_entry(&Entry::Error(error.to_string()));
            self.edit_ollama_endpoint(value, "login failed");
            return Ok(());
        }
        self.insert_entry(&Entry::Notice(format!("saved Ollama endpoint {base_url}")));
        let Some(target) = catalog::login_target_for_provider("ollama") else {
            self.insert_entry(&Entry::Error(
                "saved Ollama endpoint, but it has no API key login".into(),
            ));
            self.set_status("login failed");
            return Ok(());
        };
        self.input_ui
            .set_composer(ComposerMode::SecretInput(SecretInput::optional(target)));
        self.set_status("enter API key or leave blank");
        Ok(())
    }

    pub(super) fn cancel_ollama_endpoint(&mut self) {
        self.input_ui.set_composer(ComposerMode::Input);
        self.set_status("login cancelled");
    }

    fn persist_ollama_endpoint(&mut self, base_url: &str) -> anyhow::Result<()> {
        self.info
            .services
            .config_repository
            .update(|config| config.providers.set_endpoint("ollama", base_url))?
    }
}
