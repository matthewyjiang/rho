use super::*;

impl App {
    /// Makes `target`'s auth profile the config's when its provider is the
    /// config's default, and changes nothing else there. All a login
    /// writes: the session's own model is no default.
    pub(super) fn persist_default_login_auth(&mut self, target: &LoginTarget) {
        if target.auth == rho_providers::provider::KEYLESS_AUTH {
            return;
        }
        let result = self.info.services.config_repository.update(|config| {
            if config.provider == target.provider {
                config.auth = target.auth.clone();
            }
        });
        if let Err(err) = result {
            self.insert_entry(&Entry::Error(format!(
                "stored credentials, but saving auth mode failed: {err}"
            )));
        }
    }
}
