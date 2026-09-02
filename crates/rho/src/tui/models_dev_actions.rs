//! `/config` action to force-refresh the models.dev catalog snapshot,
//! plus the `/refresh-models` shortcut that also refreshes provider lists.

use ratatui::DefaultTerminal;
use rho_providers::model::force_refresh_models_dev_catalog;

use super::{provider_picker, App, Entry, InteractiveRuntime};

impl App {
    pub(super) async fn refresh_models_dev_catalog(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<usize> {
        self.set_status("refreshing models.dev catalog");
        terminal.draw(|frame| self.draw(frame))?;
        let written = force_refresh_models_dev_catalog().await;
        if written == 0 {
            self.insert_entry(&Entry::Error(
                "failed to refresh the models.dev catalog".into(),
            ));
            self.set_status("models.dev catalog refresh failed");
        } else {
            self.insert_entry(&Entry::Notice(format!(
                "refreshed models.dev catalog: {written} models"
            )));
            self.set_status("models.dev catalog refresh complete");
            self.start_model_metadata_fetch(agent);
        }
        Ok(written)
    }

    /// `/refresh-models` shortcut for `/config` → Providers → Refresh model
    /// lists (all) + Refresh models.dev catalog.
    ///
    /// Runs both refreshes in sequence so one command keeps cached provider
    /// models and models.dev metadata in sync.
    pub(super) async fn execute_refresh_models_command(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.refresh_model_lists(provider_picker::ALL_REFRESHABLE_PROVIDERS, terminal)
            .await?;
        let written = self.refresh_models_dev_catalog(terminal, agent).await?;
        if written > 0 {
            self.set_status("model refresh complete");
        }
        Ok(())
    }
}
