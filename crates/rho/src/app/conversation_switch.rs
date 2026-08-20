//! Conversation-model change on an assembled SDK session.
//!
//! Interactive TUI wraps this with display history, MCP sampling, and run
//! transition. ACP calls it directly. Both must keep provider, reasoning,
//! compaction, the switch notice, and delegated selection on one path.

use std::sync::Arc;

use rho_providers::model::{ReasoningCapabilities, ReasoningRequestSource, ReasoningResolution};
use rho_providers::reasoning::ReasoningLevel;
use rho_sdk::{
    model::{handoff::HandoffReport, Message},
    provider::ModelProvider,
    Error, Session,
};

use super::runtime_builder::build_compaction;
use crate::{
    compaction::CompactionConfig,
    model_identity::PromptModel,
    prompt::{model_switch_context, ModelSwitchKind},
    tools::sdk_registry::AppToolSet,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelSwitchReasoningResolution {
    pub(crate) effective: ReasoningLevel,
    pub(crate) source: ReasoningRequestSource,
}

pub(crate) fn resolve_model_switch_reasoning(
    capabilities: &ReasoningCapabilities,
    requested: ReasoningLevel,
    source: ReasoningRequestSource,
) -> Result<ModelSwitchReasoningResolution, ReasoningLevel> {
    let resolution = capabilities.resolve(requested, source);
    match resolution {
        ReasoningResolution::UnsupportedExplicit(requested) => Err(requested),
        ReasoningResolution::Normalized { effective, .. } => Ok(ModelSwitchReasoningResolution {
            effective,
            source: ReasoningRequestSource::PersistedOrDefault,
        }),
        ReasoningResolution::Exact(effective) | ReasoningResolution::Unknown(effective) => {
            Ok(ModelSwitchReasoningResolution { effective, source })
        }
        ReasoningResolution::NotConfigurable => Ok(ModelSwitchReasoningResolution {
            effective: requested,
            source,
        }),
    }
}

pub(crate) struct ConversationSwitch<'a> {
    pub(crate) session: &'a Session,
    pub(crate) tools: &'a AppToolSet,
    pub(crate) new_provider: Arc<dyn ModelProvider>,
    pub(crate) new_reasoning: ReasoningLevel,
    pub(crate) auth: &'a str,
    pub(crate) compaction: CompactionConfig,
    pub(crate) context_window: Option<u64>,
    pub(crate) previous_context_window: Option<u64>,
    pub(crate) usage_recording: rho_sdk::ProviderRequestUsageRecording,
}

pub(crate) fn apply_conversation_switch(
    switch: ConversationSwitch<'_>,
) -> Result<HandoffReport, Error> {
    let previous_provider = switch.session.provider();
    let previous_reasoning = switch.session.reasoning_level();
    let previous_prompt_model = PromptModel::from_sdk_identity(&previous_provider.identity());
    let session_started = !switch.session.history().is_empty();

    switch.session.set_reasoning_level(switch.new_reasoning)?;
    let report = match switch
        .session
        .replace_provider(Arc::clone(&switch.new_provider))
    {
        Ok(report) => report,
        Err(error) => {
            let _ = switch.session.set_reasoning_level(previous_reasoning);
            return Err(error);
        }
    };

    if let Err(error) = refresh_session_compaction(&switch) {
        return Err(restore_after_failed_step(
            switch.session,
            previous_provider,
            previous_reasoning,
            error,
            RestoreCompaction::Skip,
            None,
        ));
    }

    let current_prompt_model = PromptModel::from_sdk_identity(&switch.new_provider.identity());
    if session_started && current_prompt_model != previous_prompt_model {
        let (context, _) =
            model_switch_context(ModelSwitchKind::Conversation, &current_prompt_model);
        if let Err(error) = switch.session.append_message(Message::user_text(context)) {
            let error = Error::InvalidConfiguration {
                message: format!(
                    "could not record the conversation model switch for the model: {error}"
                ),
            };
            return Err(restore_after_failed_step(
                switch.session,
                previous_provider,
                previous_reasoning,
                error,
                RestoreCompaction::Required,
                Some(&switch),
            ));
        }
    }

    if let Some(manager) = switch.tools.subagents() {
        let identity = switch.new_provider.identity();
        manager.update_selection(
            &identity.provider,
            &identity.model,
            switch.new_reasoning,
            switch.auth,
        );
    }
    Ok(report)
}

fn refresh_session_compaction(switch: &ConversationSwitch<'_>) -> Result<(), Error> {
    let (compactor, policy) = build_compaction(
        Arc::clone(&switch.new_provider),
        switch.tools.tools(),
        switch.new_reasoning,
        switch.compaction.clone(),
        switch.context_window,
        switch.usage_recording.clone(),
    );
    switch
        .session
        .set_compaction(Some(Arc::new(compactor)), policy)
}

fn restore_after_failed_step(
    session: &Session,
    previous_provider: Arc<dyn ModelProvider>,
    previous_reasoning: ReasoningLevel,
    primary: Error,
    compaction: RestoreCompaction,
    switch: Option<&ConversationSwitch<'_>>,
) -> Error {
    if let Err(rollback_error) = session.set_reasoning_level(previous_reasoning) {
        return Error::InvalidConfiguration {
            message: format!(
                "{primary}; also failed to restore the previous reasoning: {rollback_error}"
            ),
        };
    }
    if let Err(rollback_error) = session.replace_provider(previous_provider) {
        return Error::InvalidConfiguration {
            message: format!(
                "{primary}; also failed to restore the previous provider: {rollback_error}"
            ),
        };
    }
    if matches!(compaction, RestoreCompaction::Required) {
        let Some(switch) = switch else {
            return primary;
        };
        let (compactor, policy) = build_compaction(
            session.provider(),
            switch.tools.tools(),
            previous_reasoning,
            switch.compaction.clone(),
            switch.previous_context_window,
            switch.usage_recording.clone(),
        );
        if let Err(refresh_error) = session.set_compaction(Some(Arc::new(compactor)), policy) {
            return Error::InvalidConfiguration {
                message: format!(
                    "{primary}; could not restore compaction for the previous provider: {refresh_error}"
                ),
            };
        }
    }
    primary
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestoreCompaction {
    Skip,
    Required,
}

#[cfg(test)]
#[path = "conversation_switch_tests.rs"]
mod tests;
