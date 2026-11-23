//! `/refresh-models`, and what the TUI shows of the model lists the daemon
//! fetches: the daemon owns the provider lists and the models.dev catalog
//! (`rho/model/refresh`), so this process only reports each fetch and
//! reads what the lists changed.

use rho_protocol::{ModelListKind, ModelListRefresh, ModelRefreshOutcome};

use super::{App, Entry};
use crate::tui::DefaultTerminal;

impl App {
    /// `/refresh-models`: the daemon fetches every signed-in provider's
    /// model list, then the models.dev catalog, so cached provider models
    /// and models.dev metadata stay in step.
    pub(super) async fn execute_refresh_models_command(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.set_status("refreshing model lists");
        self.paint(terminal)?;
        let response = match self.info.session.daemon.refresh_models(None).await {
            Ok(response) => response,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not refresh the model lists: {error:#}"
                )));
                self.set_status("model refresh failed");
                return Ok(());
            }
        };
        self.show_model_list_refreshes(&response.lists);
        let catalog_failed = match response.catalog {
            Some(ModelRefreshOutcome::Refreshed { models }) => {
                self.insert_entry(&Entry::Notice(format!(
                    "refreshed models.dev catalog: {models} models"
                )));
                false
            }
            Some(ModelRefreshOutcome::Failed { message }) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not refresh the models.dev catalog: {message}"
                )));
                true
            }
            // `Unknown`, or none: a newer daemon's report.
            _ => false,
        };
        self.model_lists_refreshed();
        self.set_status(if response.lists.is_empty() {
            "no refreshable providers are configured. open Config > Log in to provider to add one."
        } else if catalog_failed {
            "models.dev catalog refresh failed"
        } else {
            "model refresh complete"
        });
        Ok(())
    }

    /// Shows how each fetch of a provider model list went.
    pub(super) fn show_model_list_refreshes(&mut self, lists: &[ModelListRefresh]) {
        for refresh in lists {
            let provider = &refresh.provider;
            let list = match refresh.list {
                ModelListKind::Chat => "model list",
                ModelListKind::Decision => "decision models",
                // A list a newer daemon fetched.
                _ => "models",
            };
            match &refresh.outcome {
                ModelRefreshOutcome::Refreshed { models } => self.insert_entry(&Entry::Notice(
                    format!("refreshed {provider} {list}: {models} models"),
                )),
                ModelRefreshOutcome::Failed { message } => self.insert_entry(&Entry::Error(
                    format!("could not refresh {provider} {list}: {message}"),
                )),
                // An outcome a newer daemon reports.
                _ => {}
            }
        }
    }

    /// Reads again what the daemon's fetched lists changed for this
    /// process: the model names and capabilities its chrome shows, and the
    /// models a session not opened yet offers.
    pub(super) fn model_lists_refreshed(&mut self) {
        rho_providers::model::forget_model_display_names();
        self.info.session.daemon.model_lists_changed();
        self.refresh_catalog_chrome();
        // Options that did not change bring no event to recompute it.
        self.model_metadata = super::reasoning_metadata::cached_metadata(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        )
        .map(|(metadata, _)| metadata);
    }
}
