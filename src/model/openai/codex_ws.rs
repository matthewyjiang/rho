use serde_json::{json, Value};

use crate::model::ModelError;

/// Session-scoped continuation state for a future Codex WebSocket transport.
///
/// rho does not currently know a supported Codex WebSocket endpoint or message
/// envelope, so this module intentionally models only compatibility checks and
/// delta request construction. The provider still sends normal SSE Responses
/// requests and can use this state to reset stale continuation metadata when a
/// full-request fallback shows that the next request is incompatible.
#[derive(Debug, Default)]
pub(super) struct CodexContinuationState {
    snapshot: Option<CodexContinuationSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
struct CodexContinuationSnapshot {
    response_id: String,
    key: CodexContinuationKey,
    input: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CodexContinuationCandidate {
    key: CodexContinuationKey,
    input: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
struct CodexContinuationKey {
    model: String,
    instructions: String,
    tools: Vec<Value>,
    tool_choice: Option<Value>,
    reasoning: Option<Value>,
    prompt_cache_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum CodexContinuationPlan {
    Full {
        reason: CodexContinuationFullReason,
    },
    Delta {
        previous_response_id: String,
        input: Vec<Value>,
        body: Value,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum CodexContinuationFullReason {
    MissingPreviousResponse,
    EmptyDelta,
    Incompatible(CodexContinuationResetReason),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum CodexContinuationResetReason {
    ModelChanged,
    InstructionsChanged,
    ToolsChanged,
    ToolChoiceChanged,
    ReasoningChanged,
    PromptCacheKeyChanged,
    HistoryRewritten,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct CodexSseFallback {
    pub(super) planned_delta: bool,
    pub(super) reset_reason: Option<CodexContinuationResetReason>,
}

impl CodexContinuationCandidate {
    pub(super) fn from_responses_body(body: &Value) -> Result<Self, ModelError> {
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| ModelError::InvalidResponse("Codex body missing model".into()))?
            .to_string();
        let instructions = body
            .get("instructions")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let input = body
            .get("input")
            .and_then(Value::as_array)
            .ok_or_else(|| ModelError::InvalidResponse("Codex body missing input".into()))?
            .clone();
        let tools = body
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let tool_choice = body.get("tool_choice").cloned();
        let reasoning = body.get("reasoning").cloned();
        let prompt_cache_key = body
            .get("prompt_cache_key")
            .and_then(Value::as_str)
            .map(str::to_string);

        Ok(Self {
            key: CodexContinuationKey {
                model,
                instructions,
                tools,
                tool_choice,
                reasoning,
                prompt_cache_key,
            },
            input,
        })
    }

    fn delta_body(&self, previous_response_id: &str, input: Vec<Value>) -> Value {
        let mut body = json!({
            "model": self.key.model,
            "instructions": self.key.instructions,
            "input": input,
            "previous_response_id": previous_response_id,
            "store": false,
            "stream": true,
        });

        if let Some(prompt_cache_key) = &self.key.prompt_cache_key {
            body["prompt_cache_key"] = json!(prompt_cache_key);
        }
        if !self.key.tools.is_empty() {
            body["tools"] = json!(self.key.tools);
        }
        if let Some(tool_choice) = &self.key.tool_choice {
            body["tool_choice"] = tool_choice.clone();
        }
        if let Some(reasoning) = &self.key.reasoning {
            body["reasoning"] = reasoning.clone();
        }

        body
    }
}

impl CodexContinuationState {
    pub(super) fn plan_delta(
        &self,
        candidate: &CodexContinuationCandidate,
    ) -> CodexContinuationPlan {
        let Some(snapshot) = &self.snapshot else {
            return CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::MissingPreviousResponse,
            };
        };
        if let Some(reason) = incompatible_reason(&snapshot.key, &candidate.key) {
            return CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::Incompatible(reason),
            };
        }
        if !input_has_prefix(&candidate.input, &snapshot.input) {
            return CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::Incompatible(
                    CodexContinuationResetReason::HistoryRewritten,
                ),
            };
        }
        let delta = candidate.input[snapshot.input.len()..].to_vec();
        if delta.is_empty() {
            return CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::EmptyDelta,
            };
        }
        CodexContinuationPlan::Delta {
            previous_response_id: snapshot.response_id.clone(),
            input: delta.clone(),
            body: candidate.delta_body(&snapshot.response_id, delta),
        }
    }

    /// Prepare for the current production path: full-context SSE fallback.
    ///
    /// If a future continuation snapshot no longer matches the next full
    /// request, clear it before the fallback is sent so a later WebSocket
    /// implementation cannot accidentally continue across compaction, model,
    /// reasoning, prompt-cache, or tool changes.
    pub(super) fn prepare_sse_fallback(
        &mut self,
        candidate: &CodexContinuationCandidate,
    ) -> CodexSseFallback {
        let plan = self.plan_delta(candidate);
        let reset_reason = match plan {
            CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::Incompatible(reason),
            } => {
                self.reset();
                Some(reason)
            }
            CodexContinuationPlan::Full {
                reason:
                    CodexContinuationFullReason::MissingPreviousResponse
                    | CodexContinuationFullReason::EmptyDelta,
            }
            | CodexContinuationPlan::Delta { .. } => None,
        };

        CodexSseFallback {
            planned_delta: matches!(
                self.plan_delta(candidate),
                CodexContinuationPlan::Delta { .. }
            ),
            reset_reason,
        }
    }

    pub(super) fn record_success(
        &mut self,
        candidate: &CodexContinuationCandidate,
        response_id: Option<String>,
    ) {
        let Some(response_id) = response_id.filter(|id| !id.is_empty()) else {
            self.reset();
            return;
        };
        self.snapshot = Some(CodexContinuationSnapshot {
            response_id,
            key: candidate.key.clone(),
            input: candidate.input.clone(),
        });
    }

    pub(super) fn reset(&mut self) {
        self.snapshot = None;
    }
}

fn incompatible_reason(
    previous: &CodexContinuationKey,
    next: &CodexContinuationKey,
) -> Option<CodexContinuationResetReason> {
    if previous.model != next.model {
        return Some(CodexContinuationResetReason::ModelChanged);
    }
    if previous.instructions != next.instructions {
        return Some(CodexContinuationResetReason::InstructionsChanged);
    }
    if previous.tools != next.tools {
        return Some(CodexContinuationResetReason::ToolsChanged);
    }
    if previous.tool_choice != next.tool_choice {
        return Some(CodexContinuationResetReason::ToolChoiceChanged);
    }
    if previous.reasoning != next.reasoning {
        return Some(CodexContinuationResetReason::ReasoningChanged);
    }
    if previous.prompt_cache_key != next.prompt_cache_key {
        return Some(CodexContinuationResetReason::PromptCacheKeyChanged);
    }
    None
}

fn input_has_prefix(input: &[Value], prefix: &[Value]) -> bool {
    input.len() >= prefix.len()
        && input
            .iter()
            .zip(prefix.iter())
            .all(|(input, prefix)| input == prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(input: Vec<Value>) -> Value {
        json!({
            "model": "gpt-5-codex",
            "instructions": "system",
            "input": input,
            "store": false,
            "stream": true,
            "prompt_cache_key": "rho:session",
            "tools": [{"type":"function","name":"read","parameters":{"type":"object"}}],
            "tool_choice": "auto",
            "reasoning": {"effort":"low","summary":"auto"},
        })
    }

    fn candidate(input: Vec<Value>) -> CodexContinuationCandidate {
        CodexContinuationCandidate::from_responses_body(&body(input)).unwrap()
    }

    #[test]
    fn builds_delta_body_when_next_input_extends_previous_input() {
        let first = candidate(vec![json!({"role":"user","content":"one"})]);
        let second = candidate(vec![
            json!({"role":"user","content":"one"}),
            json!({"role":"assistant","content":"two"}),
            json!({"role":"user","content":"three"}),
        ]);
        let mut state = CodexContinuationState::default();
        state.record_success(&first, Some("resp_1".into()));

        let plan = state.plan_delta(&second);

        let CodexContinuationPlan::Delta {
            previous_response_id,
            input,
            body,
        } = plan
        else {
            panic!("expected delta plan");
        };
        assert_eq!(previous_response_id, "resp_1");
        assert_eq!(
            input,
            vec![
                json!({"role":"assistant","content":"two"}),
                json!({"role":"user","content":"three"}),
            ]
        );
        assert_eq!(body["previous_response_id"], "resp_1");
        assert_eq!(body["input"], json!(input));
        assert_eq!(body["model"], "gpt-5-codex");
        assert_eq!(body["prompt_cache_key"], "rho:session");
        assert_eq!(body["tools"][0]["name"], "read");
        assert_eq!(body["reasoning"], json!({"effort":"low","summary":"auto"}));
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn falls_back_to_full_request_without_previous_response_id() {
        let state = CodexContinuationState::default();
        let plan = state.plan_delta(&candidate(vec![json!({"role":"user","content":"one"})]));

        assert_eq!(
            plan,
            CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::MissingPreviousResponse
            }
        );
    }

    #[test]
    fn resets_when_history_is_rewritten_by_compaction() {
        let first = candidate(vec![
            json!({"role":"user","content":"old"}),
            json!({"role":"assistant","content":"answer"}),
        ]);
        let compacted = candidate(vec![
            json!({"role":"user","content":"summary of old conversation"}),
            json!({"role":"user","content":"new"}),
        ]);
        let mut state = CodexContinuationState::default();
        state.record_success(&first, Some("resp_1".into()));

        let fallback = state.prepare_sse_fallback(&compacted);

        assert!(!fallback.planned_delta);
        assert_eq!(
            fallback.reset_reason,
            Some(CodexContinuationResetReason::HistoryRewritten)
        );
        assert_eq!(
            state.plan_delta(&compacted),
            CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::MissingPreviousResponse
            }
        );
    }

    #[test]
    fn resets_when_tools_change() {
        let first = candidate(vec![json!({"role":"user","content":"one"})]);
        let mut changed_body = body(vec![
            json!({"role":"user","content":"one"}),
            json!({"role":"user","content":"two"}),
        ]);
        changed_body["tools"] = json!([{ "type":"function", "name":"write" }]);
        let changed = CodexContinuationCandidate::from_responses_body(&changed_body).unwrap();
        let mut state = CodexContinuationState::default();
        state.record_success(&first, Some("resp_1".into()));

        let fallback = state.prepare_sse_fallback(&changed);

        assert_eq!(
            fallback.reset_reason,
            Some(CodexContinuationResetReason::ToolsChanged)
        );
        assert_eq!(
            state.plan_delta(&changed),
            CodexContinuationPlan::Full {
                reason: CodexContinuationFullReason::MissingPreviousResponse
            }
        );
    }

    #[test]
    fn resets_when_model_changes() {
        let first = candidate(vec![json!({"role":"user","content":"one"})]);
        let mut changed_body = body(vec![
            json!({"role":"user","content":"one"}),
            json!({"role":"user","content":"two"}),
        ]);
        changed_body["model"] = json!("gpt-5-codex-alt");
        let changed = CodexContinuationCandidate::from_responses_body(&changed_body).unwrap();
        let mut state = CodexContinuationState::default();
        state.record_success(&first, Some("resp_1".into()));

        let fallback = state.prepare_sse_fallback(&changed);

        assert_eq!(
            fallback.reset_reason,
            Some(CodexContinuationResetReason::ModelChanged)
        );
    }

    #[test]
    fn resets_when_reasoning_changes() {
        let first = candidate(vec![json!({"role":"user","content":"one"})]);
        let mut changed_body = body(vec![
            json!({"role":"user","content":"one"}),
            json!({"role":"user","content":"two"}),
        ]);
        changed_body["reasoning"] = json!({"effort":"high","summary":"auto"});
        let changed = CodexContinuationCandidate::from_responses_body(&changed_body).unwrap();
        let mut state = CodexContinuationState::default();
        state.record_success(&first, Some("resp_1".into()));

        let fallback = state.prepare_sse_fallback(&changed);

        assert_eq!(
            fallback.reset_reason,
            Some(CodexContinuationResetReason::ReasoningChanged)
        );
    }
}
