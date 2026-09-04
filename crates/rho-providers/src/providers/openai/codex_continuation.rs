use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::model::{Message, ModelError, ModelResponse};

use crate::protocol::openai_responses::codex_input_items;

/// Holds the canonical boundary between a completed Responses request and the
/// next one. A continuation is valid only when the next locally generated
/// request starts with the original input plus the locally represented form of
/// the server response retained here.
#[derive(Debug, Default)]
pub(super) struct CodexContinuationState {
    snapshot: Option<CodexContinuationSnapshot>,
}

#[derive(Clone, Debug)]
pub(super) struct CodexContinuationCandidate {
    pub(super) request_properties: Value,
    pub(super) input: Vec<Value>,
}

#[derive(Clone, Debug)]
pub(super) struct CodexContinuationResponse {
    response_id: Option<String>,
    server_output_items: Vec<Value>,
    local_output_items: Option<Vec<Value>>,
}

#[derive(Clone, Debug)]
struct CodexContinuationSnapshot {
    response_id: String,
    request_properties: Value,
    request_input: Vec<Value>,
    server_output_items: Vec<Value>,
    local_output_items: Option<Vec<Value>>,
}

impl CodexContinuationCandidate {
    pub(super) fn from_responses_body(body: &Value) -> Result<Self, ModelError> {
        let input = body
            .get("input")
            .and_then(Value::as_array)
            .ok_or_else(|| ModelError::InvalidResponse("Codex body missing input".into()))?
            .clone();
        let properties = body.as_object().ok_or_else(|| {
            ModelError::InvalidResponse("Codex body must be a JSON object".into())
        })?;
        let request_properties = Value::Object(
            properties
                .iter()
                .filter(|(key, _)| !matches!(key.as_str(), "input" | "previous_response_id"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );

        Ok(Self {
            request_properties,
            input,
        })
    }

    fn continuation_body(&self, previous_response_id: &str, input: Vec<Value>) -> Value {
        let mut body = self.request_properties.clone();
        body["input"] = Value::Array(input);
        body["previous_response_id"] = Value::String(previous_response_id.into());
        body
    }
}

impl CodexContinuationResponse {
    pub(super) fn from_response(
        response: &ModelResponse,
        response_id: Option<String>,
        server_output_items: Vec<Value>,
    ) -> Self {
        let ModelResponse::Assistant(blocks) = response;
        let local_output_items =
            codex_input_items(&[Message::Assistant(blocks.clone())], &mut Vec::new()).ok();
        let local_output_items = local_output_items.filter(|local_output_items| {
            canonical_server_output_items(&server_output_items)
                .is_some_and(|server_output_items| server_output_items == *local_output_items)
        });
        // Stamp `async: true` after the canonical match so extra keys on the
        // server item do not break recognition, while stored local items still
        // match the next request's replayed input.
        let local_output_items = local_output_items.map(|mut items| {
            stamp_async_function_calls(&mut items, &server_output_items);
            items
        });
        Self {
            response_id,
            server_output_items,
            local_output_items,
        }
    }
}

fn canonical_server_output_items(items: &[Value]) -> Option<Vec<Value>> {
    let mut canonical = Vec::new();
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                let role = item.get("role")?.as_str()?;
                let content = item.get("content")?.as_array()?;
                let mut text = Vec::new();
                for content_item in content {
                    if content_item.get("type").and_then(Value::as_str) != Some("output_text") {
                        return None;
                    }
                    text.push(content_item.get("text")?.as_str()?.to_string());
                }
                canonical.push(serde_json::json!({
                    "role": role,
                    "content": text.join("\n"),
                }));
            }
            Some("function_call") => {
                let call_id = item.get("call_id")?.as_str()?;
                let name = item.get("name")?.as_str()?;
                let arguments = canonical_json_string(item.get("arguments")?.as_str()?)?;
                canonical.push(serde_json::json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    "arguments": arguments,
                }));
            }
            _ => return None,
        }
    }
    (!canonical.is_empty()).then_some(canonical)
}

fn stamp_async_function_calls(items: &mut [Value], server_items: &[Value]) {
    let async_ids = server_items
        .iter()
        .filter_map(|item| {
            (item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("async") == Some(&Value::Bool(true)))
            .then(|| item.get("call_id")?.as_str().map(str::to_owned))
            .flatten()
        })
        .collect::<BTreeSet<_>>();
    for item in items {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            continue;
        }
        let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
            continue;
        };
        if async_ids.contains(call_id) {
            item["async"] = json!(true);
        }
    }
}

fn canonical_json_string(input: &str) -> Option<String> {
    let value = serde_json::from_str(input).ok()?;
    serde_json::to_string(&canonical_json_value(value)).ok()
}

fn canonical_json_value(value: Value) -> Value {
    match value {
        Value::Array(values) => {
            Value::Array(values.into_iter().map(canonical_json_value).collect())
        }
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, canonical_json_value(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        primitive => primitive,
    }
}

impl CodexContinuationState {
    /// Returns a delta body when the preceding completed response supplies a
    /// canonical server-output baseline and the new request exactly extends
    /// rho's locally represented version of that baseline.
    ///
    /// Returns `None` after clearing a snapshot that can no longer extend, so
    /// the caller sends its full body instead. Handing back `None` rather than
    /// the full body lets the caller keep sole ownership of that body for an
    /// SSE retry, so no turn has to copy conversation history up front.
    pub(super) fn continuation_delta(
        &mut self,
        candidate: &CodexContinuationCandidate,
    ) -> Option<Value> {
        let delta = self.matching_delta(candidate);
        if delta.is_none() {
            self.reset();
        }
        delta
    }

    fn matching_delta(&self, candidate: &CodexContinuationCandidate) -> Option<Value> {
        let snapshot = self.snapshot.as_ref()?;
        if snapshot.request_properties != candidate.request_properties
            || snapshot.server_output_items.is_empty()
        {
            return None;
        }

        let local_output_items = snapshot.local_output_items.as_deref()?;
        let prefix_length = snapshot.request_input.len() + local_output_items.len();
        if candidate.input.len() <= prefix_length
            || !candidate.input.starts_with(&snapshot.request_input)
            || !candidate.input[snapshot.request_input.len()..].starts_with(local_output_items)
        {
            return None;
        }

        Some(candidate.continuation_body(
            &snapshot.response_id,
            candidate.input[prefix_length..].to_vec(),
        ))
    }

    pub(super) fn record_success(
        &mut self,
        candidate: CodexContinuationCandidate,
        response: CodexContinuationResponse,
    ) {
        let Some(response_id) = response.response_id.filter(|id| !id.is_empty()) else {
            self.reset();
            return;
        };
        self.snapshot = Some(CodexContinuationSnapshot {
            response_id,
            request_properties: candidate.request_properties,
            request_input: candidate.input,
            server_output_items: response.server_output_items,
            local_output_items: response.local_output_items,
        });
    }

    pub(super) fn reset(&mut self) {
        self.snapshot = None;
    }
}

#[cfg(test)]
#[path = "codex_continuation_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "codex_continuation_perf_tests.rs"]
mod perf_tests;
