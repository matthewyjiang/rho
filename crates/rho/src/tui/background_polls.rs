//! Background polling for model metadata and update notices.

use futures_util::FutureExt;
use rho_providers::model::models_dev::fetch_model_metadata;
use rho_providers::model::ReasoningRequestSource::PersistedOrDefault;

use super::{reasoning_metadata, App, Entry, InteractiveRuntime};

impl App {
    pub(super) fn poll_update_notice(&mut self) {
        let Some(handle) = self.pending_update_notice.as_mut() else {
            return;
        };
        let Some(result) = handle.now_or_never() else {
            return;
        };
        self.pending_update_notice = None;
        if let Ok(Some(notice)) = result {
            self.info.services.update_notice = Some(notice);
        }
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
        if let Some(handle) = self.pending_model_metadata.take() {
            handle.abort();
        }
        self.pending_model_metadata_reasoning = None;
        if let Some((metadata, metadata_is_current)) = reasoning_metadata::cached_metadata(
            &self.info.runtime.provider,
            &self.info.runtime.model,
        ) {
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
        let provider = self.info.runtime.provider.clone();
        let model = self.info.runtime.model.clone();
        self.pending_model_metadata_reasoning = Some((
            self.info.runtime.reasoning,
            self.info.runtime.reasoning_source,
        ));
        self.pending_model_metadata = Some(tokio::spawn(async move {
            fetch_model_metadata(&provider, &model).await
        }));
    }

    pub(super) async fn poll_model_metadata_fetch(&mut self, agent: &mut InteractiveRuntime) {
        let Some(handle) = self.pending_model_metadata.as_mut() else {
            return;
        };
        if !handle.is_finished() {
            return;
        }
        if let Some(handle) = self.pending_model_metadata.take() {
            let reasoning_at_fetch_start = self.pending_model_metadata_reasoning.take();
            if let Some(Ok(Some(metadata))) = handle.now_or_never() {
                if !self.apply_context_window(agent, metadata.display_context_window()) {
                    // Keep prior metadata until a later fetch can apply cleanly.
                    return;
                }
                let capabilities = metadata.reasoning_capabilities();
                let resolved = reasoning_metadata::resolve_fetched_reasoning(
                    &capabilities,
                    self.info.runtime.reasoning,
                    reasoning_at_fetch_start,
                );
                let reasoning = resolved.effective;
                if let Some(requested) = resolved.rejected {
                    self.insert_entry(&Entry::Error(format!(
                        "reasoning level '{requested}' is not supported by {}/{}; restored '{reasoning}'",
                        self.info.runtime.provider, self.info.runtime.model
                    )));
                }
                let provider_updated = match self
                    .build_provider_for_selection(
                        &self.info.runtime.provider,
                        &self.info.runtime.model,
                        reasoning,
                        &self.info.runtime.auth,
                    )
                    .await
                {
                    Ok(provider) => {
                        match agent.replace_provider(provider, reasoning, &self.info.runtime.auth) {
                            Ok(_) => true,
                            Err(err) => {
                                self.insert_entry(&Entry::Error(format!(
                                    "could not apply model reasoning metadata: {err}"
                                )));
                                false
                            }
                        }
                    }
                    Err(err) => {
                        self.insert_entry(&Entry::Error(format!(
                            "could not apply model reasoning metadata: {err}"
                        )));
                        false
                    }
                };
                if provider_updated && reasoning != self.info.runtime.reasoning {
                    self.info.set_reasoning(reasoning, PersistedOrDefault);
                    if let Err(err) = self.info.services.config_repository.update(|config| {
                        config.reasoning = reasoning;
                    }) {
                        self.insert_entry(&Entry::Error(format!(
                            "could not save normalized reasoning: {err}"
                        )));
                    }
                }
                self.model_metadata = Some(metadata);
            }
        }
    }
}
