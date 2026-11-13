use super::{App, Entry};

impl App {
    /// Says the session's model has no fast mode to turn on.
    pub(super) fn refuse_fast_mode(&mut self) {
        let provider = &self.info.runtime.provider;
        let model = &self.info.runtime.model;
        let message = if provider == "xai" && model == rho_providers::providers::fast_mode::GROK_4_7
        {
            "fast mode for xai/grok-4.7 requires SpaceXAI OAuth".to_string()
        } else {
            format!("fast mode is not available for {provider}/{model}")
        };
        self.insert_entry(&Entry::Error(message));
        self.set_status("fast mode unavailable");
    }

    pub(super) fn report_fast_mode(&mut self, enabled: bool, supported: bool) {
        let (mode, detail) = match (enabled, supported) {
            (true, true) => ("on", "faster responses at a higher credit rate"),
            (true, false) => (
                "on",
                "inactive because the current model does not support it",
            ),
            (false, _) => ("off", "standard response speed and credit rate"),
        };
        self.set_status(format!("fast mode is {mode}: {detail}"));
    }
}
