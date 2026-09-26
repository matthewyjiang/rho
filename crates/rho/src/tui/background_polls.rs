//! Startup hydration and model metadata fetches.

use rho_providers::model::{
    models_dev::{custom_model_id_catalog_miss, fetch_model_metadata, CatalogLookupMiss},
    ModelMetadata, ReasoningRequestSource,
};
use rho_sdk::ReasoningLevel;
use tokio::task::JoinError;

use super::{
    background_tasks::{OnCancel, SessionOutput, TaskId},
    reasoning_metadata, App, ComposerMode, Entry, InteractiveRuntime, StatusSource,
};

#[cfg(test)]
#[path = "background_polls_tests.rs"]
mod tests;

impl App {
    pub(super) async fn poll_startup_hydrates(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let pending = agent.mcp_connect_pending();
        let changed = agent.poll_startup_hydrates().await?;
        if !changed {
            return Ok(false);
        }
        self.mcp_report = agent.mcp_report().clone();
        self.mcp_catalog = agent.mcp_catalog().clone();
        if pending && !agent.mcp_connect_pending() {
            if self.status_source == StatusSource::McpConnecting {
                self.set_status_quiet("");
            }
            self.clear_mcp_connecting_activity();
        }
        if matches!(
            self.input_ui.composer(),
            ComposerMode::Picker(picker) if picker.is_mcp_inventory()
        ) {
            let _ = self.execute_mcp_command();
        }
        Ok(true)
    }

    /// Adopt the Herdr graphics probe `tui::run` started before the app.
    pub(super) fn track_herdr_graphics(
        &mut self,
        handle: tokio::task::JoinHandle<crate::herdr::HerdrGraphicsCapability>,
    ) {
        self.tasks
            .track(TaskId::HerdrGraphics, handle, OnCancel::Await, |result| {
                SessionOutput::HerdrGraphics(result).into()
            });
    }

    /// Returns whether the runtime accepted the context window.
    fn apply_context_window(
        &mut self,
        agent: &mut InteractiveRuntime,
        context_window: Option<u64>,
    ) -> bool {
        if let Err(err) = agent.set_context_window(context_window) {
            self.insert_entry(&Entry::Error(format!(
                "could not apply the model context window: {err}"
            )));
            return false;
        }
        true
    }

    pub(super) fn start_model_metadata_fetch(&mut self, agent: &mut InteractiveRuntime) {
        self.tasks.abort(|id| *id == TaskId::ModelMetadata);
        let provider = self.info.runtime.provider.clone();
        let model = rho_providers::providers::fast_mode::request_model(
            &provider,
            &self.info.runtime.model,
            &self.info.runtime.auth,
            self.info.runtime.fast_mode_active(),
        )
        .to_string();
        if let Some((metadata, metadata_is_current)) =
            reasoning_metadata::cached_metadata(&provider, &model)
        {
            if self.apply_context_window(agent, metadata.display_context_window()) {
                let reasoning_metadata_complete = metadata.reasoning_metadata_complete;
                self.model_metadata = Some(metadata);
                if reasoning_metadata_complete && metadata_is_current {
                    return;
                }
            }
            // Failed apply: leave any prior cache alone and fall through to fetch.
        } else {
            let _ = self.apply_context_window(agent, None);
            self.model_metadata = None;
        }
        let reasoning_at_start = (
            self.info.runtime.reasoning,
            self.info.runtime.reasoning_source,
        );
        self.tasks.spawn(
            TaskId::ModelMetadata,
            async move { fetch_model_metadata(&provider, &model).await },
            move |metadata| {
                SessionOutput::ModelMetadata {
                    reasoning_at_start,
                    metadata,
                }
                .into()
            },
        );
    }

    /// Apply a finished metadata fetch. The dispatch point holds it while
    /// the session is busy.
    pub(super) async fn apply_model_metadata(
        &mut self,
        agent: &mut InteractiveRuntime,
        reasoning_at_start: (ReasoningLevel, ReasoningRequestSource),
        metadata: Result<Option<ModelMetadata>, JoinError>,
    ) {
        let Ok(Some(metadata)) = metadata else {
            self.warn_custom_model_id_catalog_miss();
            return;
        };
        if !self.apply_context_window(agent, metadata.display_context_window()) {
            // Keep prior metadata until a later fetch can apply cleanly.
            return;
        }
        self.apply_fetched_reasoning(
            agent,
            &metadata.reasoning_capabilities(),
            Some(reasoning_at_start),
        )
        .await;
        self.model_metadata = Some(metadata);
    }

    fn warn_custom_model_id_catalog_miss(&mut self) {
        let provider = self.info.runtime.provider.as_str();
        let model = self.info.runtime.model.as_str();
        let Some(miss) = custom_model_id_catalog_miss(provider, model) else {
            return;
        };
        let message = match miss {
            CatalogLookupMiss::BareModelId => format!(
                "{provider}/{model} has no models.dev metadata: catalog_mode = \"model-id\" needs a provider/model id"
            ),
            CatalogLookupMiss::MissingRow {
                source_provider,
                source_model,
            } => format!(
                "{provider}/{model} has no models.dev metadata for {source_provider}/{source_model}"
            ),
        };
        self.insert_entry(&Entry::Notice(message));
        self.set_status("models.dev catalog miss");
    }
}
