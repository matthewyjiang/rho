//! A text model as a [`DecisionModel`], run as one of Rho's internal agents
//! so the call records usage like any other internal agent.

use std::path::Path;

use rho_providers::reasoning::ReasoningLevel;
use rho_sdk::{
    decision::{
        text::{input, parse_answers, system_prompt, AnswerStyle},
        DecisionError, DecisionFuture, DecisionModel, DecisionRequest,
    },
    provider::ModelProvider,
    CancellationToken, ProviderRequestUsageRecording, SessionId,
};

use crate::agent::{
    internal_definition, run_one_shot_with_provider, OneShotAgentRequest, PromptPolicy,
};

/// Each request is one one-shot call laid out as
/// [`rho_sdk::decision::text`] describes, with the request's instructions in
/// place of the agent's prompt. It reports no probabilities and leaves sizing
/// the state to the caller.
pub(crate) struct TextModel<'a> {
    pub provider: &'a dyn ModelProvider,
    /// Internal agent whose definition the call runs with.
    pub agent_id: &'static str,
    pub usage_purpose: &'static str,
    pub reasoning: ReasoningLevel,
    pub style: AnswerStyle,
    pub session_id: &'a SessionId,
    pub workspace_path: &'a Path,
    pub usage_recording: ProviderRequestUsageRecording,
}

impl DecisionModel for TextModel<'_> {
    fn decide<'a>(
        &'a self,
        request: DecisionRequest<'a>,
        cancellation: &'a CancellationToken,
    ) -> DecisionFuture<'a> {
        Box::pin(async move {
            request.check().map_err(DecisionError::InvalidRequest)?;
            let mut definition = internal_definition(self.agent_id).clone();
            definition.prompt = PromptPolicy::Replace(system_prompt(request.instructions));
            let result = run_one_shot_with_provider(
                self.provider,
                OneShotAgentRequest {
                    definition: &definition,
                    usage_purpose: self.usage_purpose,
                    reasoning: Some(self.reasoning),
                    input: input(request, self.style),
                    cancellation: cancellation.clone(),
                    session_id: self.session_id,
                    workspace_path: self.workspace_path,
                },
                self.usage_recording.clone(),
                /*updates*/ None,
            )
            .await
            .map_err(|error| DecisionError::Model(error.into()))?;
            parse_answers(&result.texts.join("\n"), request.questions, self.style)
        })
    }
}
