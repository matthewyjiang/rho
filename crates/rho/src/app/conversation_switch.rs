//! Conversation-model change on an assembled SDK session.
//!
//! The session runtime calls this for every model change, also to install a
//! same-model provider rebuilt with renewed credentials, so provider,
//! reasoning, compaction, the switch notice, and delegated selection stay on
//! one path.

use std::sync::Arc;

use rho_providers::model::{ReasoningCapabilities, ReasoningRequestSource, ReasoningResolution};
use rho_providers::reasoning::ReasoningLevel;
use rho_sdk::{
    model::{handoff::HandoffReport, Message},
    provider::ModelProvider,
    Error, Session,
};

use super::runtime_builder::{build_compaction, CompactionSetup};
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
    /// Prepared before any live session mutation; loading failures leave it untouched.
    pub(crate) prepared_prompt: Option<&'a crate::prompt::SystemPrompt>,
    pub(crate) session: &'a Session,
    pub(crate) tools: &'a AppToolSet,
    pub(crate) previous_provider: Arc<dyn ModelProvider>,
    pub(crate) new_provider: Arc<dyn ModelProvider>,
    pub(crate) new_reasoning: ReasoningLevel,
    pub(crate) auth: &'a str,
    pub(crate) compaction: CompactionConfig,
    pub(crate) context_window: Option<u64>,
    pub(crate) previous_context_window: Option<u64>,
    pub(crate) usage_recording: rho_sdk::ProviderRequestUsageRecording,
    pub(crate) diagnostics: crate::diagnostics::RuntimeDiagnostics,
}

pub(crate) fn apply_conversation_switch(
    switch: ConversationSwitch<'_>,
) -> Result<HandoffReport, Error> {
    let previous_provider = Arc::clone(&switch.previous_provider);
    let previous_reasoning = switch.session.reasoning_level();
    let previous_prompt_model = PromptModel::from_sdk_identity(&previous_provider.identity());
    // The leading system prompt is setup, not conversation. Hosts such as T3
    // select a model before the first prompt; that must not inject a notice.
    let session_started = switch
        .session
        .history()
        .iter()
        .any(|message| !matches!(message, Message::System(_)));
    let previous_history = switch.session.history();

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
            &switch,
            previous_reasoning,
            error,
            RestoreCompaction::Skip,
            previous_history,
        ));
    }

    if let Some(prompt) = switch.prepared_prompt {
        if let Err(error) = replace_system_prompt(switch.session, &prompt.text) {
            return Err(restore_after_failed_step(
                &switch,
                previous_reasoning,
                error,
                RestoreCompaction::Required,
                previous_history,
            ));
        }
    }
    let current_prompt_model = PromptModel::from_sdk_identity(&switch.new_provider.identity());
    if session_started && current_prompt_model != previous_prompt_model {
        let (context, _) =
            model_switch_context(ModelSwitchKind::Conversation, &current_prompt_model);
        if let Err(error) = record_switch_notice(switch.session, context) {
            return Err(restore_after_failed_step(
                &switch,
                previous_reasoning,
                error,
                RestoreCompaction::Required,
                previous_history,
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

/// Replace the assembled leading prompt without accumulating model overlays or
/// changing conversation messages. Snapshot resumes use the same operation.
pub(crate) fn replace_system_prompt(session: &Session, text: &str) -> Result<(), Error> {
    let mut history = session.history();
    match history.first_mut() {
        Some(Message::System(prompt)) if prompt == text => return Ok(()),
        Some(Message::System(prompt)) => *prompt = text.to_owned(),
        _ => history.insert(0, Message::System(text.to_owned())),
    }
    session.replace_history(history).map(|_| ())
}

fn record_switch_notice(session: &Session, context: String) -> Result<(), Error> {
    session
        .append_message(Message::user_text(context))
        .map(|_| ())
        .map_err(|error| Error::InvalidConfiguration {
            message: format!(
                "could not record the conversation model switch for the model: {error}"
            ),
            source: None,
        })
}

fn refresh_session_compaction(switch: &ConversationSwitch<'_>) -> Result<(), Error> {
    let (compactor, policy) = build_compaction(CompactionSetup {
        provider: Arc::clone(&switch.new_provider),
        tool_specs: switch.tools.specs(),
        reasoning: switch.new_reasoning,
        compaction: switch.compaction.clone(),
        context_window: switch.context_window,
        usage_recording: switch.usage_recording.clone(),
        diagnostics: switch.diagnostics.clone(),
        recall: switch.tools.recall_store(),
        todo: Some(switch.tools.todo_state()),
        code_mode: Some(switch.tools.code_mode().clone()),
        credentials: switch.tools.credentials(),
    });
    switch
        .session
        .set_compaction(Some(Arc::new(compactor)), policy)
}

fn restore_after_failed_step(
    switch: &ConversationSwitch<'_>,
    previous_reasoning: ReasoningLevel,
    primary: Error,
    compaction: RestoreCompaction,
    previous_history: Vec<Message>,
) -> Error {
    let session = switch.session;
    let previous_provider = Arc::clone(&switch.previous_provider);
    if let Err(rollback_error) = session.set_reasoning_level(previous_reasoning) {
        return Error::InvalidConfiguration {
            message: format!(
                "{primary}; also failed to restore the previous reasoning: {rollback_error}"
            ),
            source: None,
        };
    }
    if let Err(rollback_error) = session.replace_provider(Arc::clone(&previous_provider)) {
        return Error::InvalidConfiguration {
            message: format!(
                "{primary}; also failed to restore the previous provider: {rollback_error}"
            ),
            source: None,
        };
    }
    if matches!(compaction, RestoreCompaction::Required) {
        let (compactor, policy) = build_compaction(CompactionSetup {
            provider: previous_provider,
            tool_specs: switch.tools.specs(),
            reasoning: previous_reasoning,
            compaction: switch.compaction.clone(),
            context_window: switch.previous_context_window,
            usage_recording: switch.usage_recording.clone(),
            diagnostics: switch.diagnostics.clone(),
            recall: switch.tools.recall_store(),
            todo: Some(switch.tools.todo_state()),
            code_mode: Some(switch.tools.code_mode().clone()),
            credentials: switch.tools.credentials(),
        });
        if let Err(refresh_error) = session.set_compaction(Some(Arc::new(compactor)), policy) {
            return Error::InvalidConfiguration {
                message: format!(
                    "{primary}; could not restore compaction for the previous provider: {refresh_error}"
                ),
                source: None,
            };
        }
    }
    match session.replace_history(previous_history) {
        Ok(_) => primary,
        Err(rollback) => Error::InvalidConfiguration {
            message: format!("{primary}; could not restore previous prompt/history: {rollback}"),
            source: None,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RestoreCompaction {
    Skip,
    Required,
}

#[cfg(test)]
#[path = "conversation_switch_tests.rs"]
mod tests;
