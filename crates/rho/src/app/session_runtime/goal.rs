//! Goal evaluation uses the session's history, usage ledger, and model context.

use rho_sdk::CancellationToken;

use super::SessionRuntime;
use crate::{
    agent::{effective_internal_agent_reasoning, GOAL_JUDGE_AGENT_ID},
    config::{Config, InternalAgentModelConfig},
    goal::{self, EvaluationRequest, GoalEvaluation},
};

impl SessionRuntime {
    /// Evaluate against committed history, using the configured judge or the
    /// session's current model. No frontend history or credentials are involved.
    pub(crate) async fn evaluate_goal(
        &self,
        condition: &str,
        config: &Config,
        cancellation: CancellationToken,
    ) -> anyhow::Result<GoalEvaluation> {
        let current = self.current_model();
        let configured = config
            .internal_agent_model(GOAL_JUDGE_AGENT_ID)
            .cloned()
            .unwrap_or_else(|| {
                InternalAgentModelConfig::new(current.provider, current.model, current.auth)
            });
        let reasoning = effective_internal_agent_reasoning(GOAL_JUDGE_AGENT_ID, &configured);
        let selection = configured
            .rho()
            .ok_or_else(|| anyhow::anyhow!("goal-judge requires a Rho provider model"))?;
        let history = self.built.session.history();
        goal::evaluate(
            EvaluationRequest {
                provider_name: &selection.provider,
                model: &selection.model,
                auth: &selection.auth,
                reasoning,
                condition,
                messages: &history,
                cancellation,
                session_id: self.session_id(),
                workspace_path: self.workspace_root(),
                credentials: self.build.host.environment.credentials(),
            },
            self.built.runtime.usage_recording(),
        )
        .await
        .map_err(|error| {
            // A judge may use a different provider/auth from the conversation.
            let error = crate::app::session_events::outcome::turn_error(
                &error,
                &selection.provider,
                &selection.auth,
            );
            anyhow::anyhow!(error.message)
        })
    }
}
