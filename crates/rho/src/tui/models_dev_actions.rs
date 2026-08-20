//! `/config` action to force-refresh the models.dev catalog snapshot.

use ratatui::DefaultTerminal;
use rho_providers::model::force_refresh_models_dev_catalog;

use super::{App, Entry, InteractiveRuntime};

impl App {
    pub(super) async fn refresh_models_dev_catalog(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
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
        Ok(())
    }
}
