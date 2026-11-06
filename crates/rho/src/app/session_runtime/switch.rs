//! Provider and model switches for one idle session: a host's model
//! selection, and the rebuild that renews stale credentials.

use std::{path::Path, sync::Arc};

use rho_providers::{
    model::{catalog::ModelSelection, models_dev, ReasoningRequestSource},
    provider::model_reference,
};

use super::{credentials::LogoutCount, model_context_window, SessionRuntime};
use crate::{
    app::conversation_switch::{
        apply_conversation_switch, resolve_model_switch_reasoning, ConversationSwitch,
        ModelSwitchReasoningPolicy, SwitchNotice,
    },
    compaction::CompactionConfig,
    config::Config,
    credential_store::{build_provider_from_config_ensuring_catalog, AppCredentialStore},
};

/// Why a model switch did not apply.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ModelSwitchError {
    /// The new model cannot keep the session's reasoning level. The
    /// requester can fix it.
    #[error("reasoning level '{level}' is not supported for '{model}'")]
    UnsupportedReasoning { level: String, model: String },
    /// Building or installing the provider, or saving the switch, failed.
    #[error("{0}")]
    Apply(anyhow::Error),
}

impl SessionRuntime {
    /// Switches the session to `selection`'s model and auth, keeping its
    /// reasoning level, then saves a call override's new selection. The host
    /// holds the runtime between prompts, so none is in flight.
    pub(crate) async fn switch_model(
        &mut self,
        selection: &ModelSelection,
        process_config: &Config,
        config_path: &Path,
    ) -> Result<(), ModelSwitchError> {
        let current = self.current_model();
        if model_reference(&selection.provider, &selection.model)
            != model_reference(&current.provider, &current.model)
            || selection.auth != current.auth
        {
            let reasoning =
                resolve_switch_reasoning(selection, self.built.session.reasoning_level())?;
            let mut config = process_config.clone();
            config.provider = selection.provider.clone();
            config.model = selection.model.clone();
            config.auth = selection.auth.clone();
            config.reasoning = reasoning;
            self.validate_model_override_switch(&config)
                .map_err(ModelSwitchError::Apply)?;
            let read = self.credentials.before_read();
            // A keyless login lives in config, which `logout` edits on disk.
            let login_config = crate::app::endpoint_login::with_disk_login(&config, config_path)
                .map_err(ModelSwitchError::Apply)?;
            let provider = build_provider_from_config_ensuring_catalog(
                &login_config,
                Arc::new(AppCredentialStore),
            )
            .await
            .map_err(|error| ModelSwitchError::Apply(error.into()))?;
            self.switch_provider(provider, &config, read)
                .map_err(ModelSwitchError::Apply)?;
        }
        self.persist_model_override().map_err(|error| {
            ModelSwitchError::Apply(anyhow::anyhow!(
                "model changed but could not save session: {error}"
            ))
        })
    }

    /// Installs an already-resolved provider on the idle session, for a
    /// model switch or renewed credentials. Kept separate from credential
    /// discovery so the session transaction can be exercised with a scripted
    /// provider. `read` is [`super::SessionCredentials::before_read`] from
    /// before `provider` was built.
    pub(crate) fn switch_provider(
        &mut self,
        provider: Arc<dyn rho_sdk::provider::ModelProvider>,
        config: &Config,
        read: LogoutCount,
    ) -> anyhow::Result<()> {
        let current = self.current_model();
        let previous_context_window = model_context_window(&current.provider, &current.model);
        let context_window = model_context_window(&config.provider, &config.model);
        let prompt_model =
            crate::model_identity::PromptModel::from_sdk_identity(&provider.identity());
        // The prompt depends only on the model, so an auth-only change (such
        // as renewed credentials) keeps the one the session already has.
        let model_changed = prompt_model
            != crate::model_identity::PromptModel::from_sdk_identity(
                &self.built.provider.identity(),
            );
        let prepared_prompt = self
            .built
            .prompt_template
            .as_ref()
            .filter(|_| model_changed)
            .map(|template| template.build(&prompt_model))
            .transpose()?;
        // HandoffReport could reach the host later; no host has a place for it yet.
        let _handoff = apply_conversation_switch(
            ConversationSwitch {
                prepared_prompt: prepared_prompt.as_ref(),
                session: &self.built.session,
                tools: &self.built.tools,
                previous_provider: Arc::clone(&self.built.provider),
                new_provider: Arc::clone(&provider),
                new_reasoning: config.reasoning,
                auth: &config.auth,
                compaction: CompactionConfig::from(config),
                context_window,
                previous_context_window,
                usage_recording: self.built.runtime.usage_recording(),
                diagnostics: self.built.diagnostics.clone(),
            },
            SwitchNotice::SessionMessage,
        )?;
        self.built.provider = provider;
        self.credentials.mark_read(read);
        if let Some(prompt) = prepared_prompt {
            self.built.prompt.adopt(
                crate::app::active_prompt::ActivePrompt::from_prepared(prompt),
                &self.built.diagnostics,
                self.built.tools.advisor(),
            );
        }
        self.auth = config.auth.clone();
        Ok(())
    }
}

fn resolve_switch_reasoning(
    selection: &ModelSelection,
    requested: rho_sdk::ReasoningLevel,
) -> Result<rho_sdk::ReasoningLevel, ModelSwitchError> {
    let capabilities =
        models_dev::current_reasoning_capabilities(&selection.provider, &selection.model);
    match resolve_model_switch_reasoning(
        &capabilities,
        requested,
        ReasoningRequestSource::PersistedOrDefault,
        ModelSwitchReasoningPolicy::PreserveExplicit,
    ) {
        Ok(resolved) => Ok(resolved.effective),
        Err(level) => Err(ModelSwitchError::UnsupportedReasoning {
            level: level.to_string(),
            model: model_reference(&selection.provider, &selection.model),
        }),
    }
}
