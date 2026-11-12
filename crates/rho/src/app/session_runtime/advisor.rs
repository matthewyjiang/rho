//! Global settings a loaded session takes at its next turn (#1720): the
//! advisor and automatic compaction.
//!
//! Advisor mode changes the advertised tool list, which the SDK cannot swap
//! on a live runtime, so registering or removing the `advisor` tool rebuilds
//! the runtime around the live conversation. The system prompt stays the
//! same for prompt-cache stability; the model learns of the change from an
//! appended context message, which the turn about to run saves with it.

use std::sync::Arc;

use rho_sdk::model::Message;

use super::{model_context_window, model_scoped_config, SessionRuntime, SettingChangeError};
use crate::{
    app::runtime_builder::{refresh_session_compaction, CompactionSetup},
    compaction::CompactionConfig,
    config::Config,
    model_identity::PromptModel,
    tools::advisor::AdvisorModel,
};

impl SessionRuntime {
    /// Applies the advisor `model` to later turns, or none when advisor
    /// mode is off or has no model. Registering or removing the tool
    /// rebuilds the runtime; a model switch while it stays registered only
    /// tells the model. Returns the notice to show when the model was told
    /// something. On error the session keeps its advisor.
    pub(crate) async fn set_advisor(
        &mut self,
        model: Option<AdvisorModel>,
        process_config: &Config,
    ) -> Result<Option<String>, SettingChangeError> {
        let Some(store) = self.built.tools.advisor().cloned() else {
            return Ok(None);
        };
        let registered = model.is_some();
        if registered == self.built.tools.advisor_registered() {
            // Compare what the notice reports, not the whole selection: a
            // reasoning-only change would otherwise announce a switch to the
            // model the advisor already uses.
            let previous = store.model();
            let identity =
                |model: &AdvisorModel| PromptModel::from_internal_agent(&model.selection);
            let previous_identity = previous.as_ref().map(identity);
            let notice = model
                .as_ref()
                .map(identity)
                .filter(|identity| previous_identity.as_ref() != Some(identity))
                .map(|identity| {
                    crate::prompt::model_switch_context(
                        crate::prompt::ModelSwitchKind::Advisor,
                        &identity,
                    )
                });
            store.set_model(model);
            let Some((context, display)) = notice else {
                return Ok(None);
            };
            if let Err(error) = self
                .built
                .session
                .append_message(Message::user_text(context))
            {
                // The store must not hold a reviewer the model was not told
                // about.
                store.set_model(previous);
                return Err(error.into());
            }
            return Ok(Some(display));
        }

        let config = model_scoped_config(&self.current_model(), process_config);
        let previous = store.model();
        self.built.tools.set_advisor_registered(registered);
        let rebuilt = self
            .rebuild_runtime(
                &config,
                self.permission.mode,
                self.permission.writes.clone(),
            )
            .await;
        if let Err(error) = rebuilt {
            self.built.tools.set_advisor_registered(!registered);
            return Err(error);
        }
        // After the rebuild, so a failed one leaves the store as it was, and
        // before the notice, so it names the model the tool consults.
        store.set_model(model);
        let told = if registered {
            let spec = self
                .built
                .tools
                .specs()
                .into_iter()
                .find(|spec| spec.name == crate::tools::advisor::TOOL_NAME);
            match (spec, store.model()) {
                (Some(spec), Some(reviewer)) => Ok(crate::prompt::advisor_enabled_context(
                    &spec,
                    &PromptModel::from_internal_agent(&reviewer.selection),
                )),
                _ => Err(SettingChangeError::Apply(anyhow::anyhow!(
                    "the advisor tool is missing after it was registered"
                ))),
            }
        } else {
            Ok(crate::prompt::advisor_disabled_context())
        }
        .and_then(|(context, display)| {
            self.built
                .session
                .append_message(Message::user_text(context))
                .map(|_| display)
                .map_err(Into::into)
        });
        let display = match told {
            Ok(display) => display,
            Err(error) => {
                // Put the old tool list back so the session never advertises
                // one the model was not told about.
                store.set_model(previous);
                self.built.tools.set_advisor_registered(!registered);
                if let Err(undo) = self
                    .rebuild_runtime(
                        &config,
                        self.permission.mode,
                        self.permission.writes.clone(),
                    )
                    .await
                {
                    tracing::warn!(%undo, "could not restore the tool list after a failed advisor change");
                }
                return Err(error);
            }
        };
        Ok(Some(display))
    }

    /// Rebuilds the session's compactor and automatic policy from
    /// `process_config`'s compaction settings, for later turns.
    pub(crate) fn set_compaction(
        &mut self,
        process_config: &Config,
    ) -> Result<(), SettingChangeError> {
        let config = model_scoped_config(&self.current_model(), process_config);
        let session = &self.built.session;
        refresh_session_compaction(
            session,
            CompactionSetup {
                provider: Arc::clone(&self.built.provider),
                tool_specs: self.built.tools.specs(),
                reasoning: session.reasoning_level(),
                compaction: CompactionConfig::from(&config),
                context_window: model_context_window(&config.provider, &config.model),
                usage_recording: self.built.runtime.usage_recording(),
                diagnostics: self.built.diagnostics.clone(),
                recall: self.built.tools.recall_store(),
                todo: Some(self.built.tools.todo_state()),
                code_mode: Some(self.built.tools.code_mode().clone()),
                credentials: self.built.tools.credentials(),
            },
        )?;
        Ok(())
    }
}
