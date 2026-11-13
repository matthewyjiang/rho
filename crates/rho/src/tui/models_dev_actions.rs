//! `/config` action to force-refresh the models.dev catalog snapshot,
//! plus the `/refresh-models` shortcut that also refreshes provider lists.

use crate::tui::DefaultTerminal;
use rho_providers::model::force_refresh_models_dev_catalog;

use super::{provider_picker, App, Entry};

impl App {
    pub(super) async fn refresh_models_dev_catalog(
        &mut self,
        terminal: &mut DefaultTerminal,
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
            self.refresh_catalog_chrome();
            self.insert_entry(&Entry::Notice(format!(
                "refreshed models.dev catalog: {written} models"
            )));
            self.set_status("models.dev catalog refresh complete");
        }
        Ok(written)
    }

    /// `/refresh-models`: refreshes every configured provider's model list,
    /// then the models.dev catalog.
    ///
    /// Runs both refreshes in sequence so one command keeps cached provider
    /// models and models.dev metadata in sync.
    pub(super) async fn execute_refresh_models_command(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.refresh_model_lists(provider_picker::ALL_REFRESHABLE_PROVIDERS, terminal)
            .await?;
        let written = self.refresh_models_dev_catalog(terminal).await?;
        // The lists are files the daemon shares; its sessions' options are
        // built from them.
        self.configure_daemon(rho_protocol::SessionChange::ModelCatalog)?;
        // Options that did not change bring no event to recompute it.
        self.model_metadata = super::reasoning_metadata::cached_metadata(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        )
        .map(|(metadata, _)| metadata);
        if written > 0 {
            self.set_status("model refresh complete");
        }
        Ok(())
    }
}
