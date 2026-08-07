//! Bounded wire envelopes for app-owned workflow lifecycle hooks.

use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

use rho_sdk::{floor_char_boundary, hooks::HookPayloadBounds};
use serde::Serialize;

use crate::hooks::config::WorkflowHookEventKind;

#[derive(Serialize)]
pub(super) struct AppHookEnvelope {
    #[serde(skip)]
    kind: WorkflowHookEventKind,
    schema_version: u32,
    event: &'static str,
    event_id: rho_sdk::HookEventId,
    timestamp_unix_ms: u64,
    identity: EmptyHookIdentity,
    workspace: EmptyHookWorkspace,
    #[serde(rename = "bounds")]
    truncation: AppHookTruncation,
    payload: BoundedWorkflowPayload,
}

impl AppHookEnvelope {
    pub(super) fn new(
        event: WorkflowHookEventKind,
        payload: WorkflowPayload<'_>,
        bounds: HookPayloadBounds,
    ) -> Result<Self, String> {
        let mut truncation = AppHookTruncation::default();
        let payload = BoundedWorkflowPayload::new(payload, bounds, &mut truncation);
        let mut envelope = Self {
            kind: event,
            schema_version: 2,
            event: event.wire_name(),
            event_id: rho_sdk::HookEventId::new(),
            timestamp_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
                .unwrap_or_default(),
            identity: EmptyHookIdentity::default(),
            workspace: EmptyHookWorkspace::default(),
            truncation,
            payload,
        };

        while encoded_len(&envelope)? > bounds.max_envelope_bytes()
            && !envelope.payload.artifact_references.is_empty()
        {
            envelope.payload.artifact_references.pop();
            envelope.truncation.record("payload.artifact_references");
        }
        let size = encoded_len(&envelope)?;
        if size > bounds.max_envelope_bytes() {
            return Err(format!(
                "workflow hook event was not delivered: {size} bytes exceeds the {} byte limit",
                bounds.max_envelope_bytes()
            ));
        }
        Ok(envelope)
    }

    pub(super) fn event(&self) -> WorkflowHookEventKind {
        self.kind
    }

    pub(super) fn wire_name(&self) -> &'static str {
        self.event
    }

    pub(super) fn to_bounded_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| error.to_string())
    }
}

fn encoded_len(value: &impl Serialize) -> Result<usize, String> {
    serde_json::to_vec(value)
        .map(|encoded| encoded.len())
        .map_err(|error| error.to_string())
}

#[derive(Default, Serialize)]
struct EmptyHookIdentity {
    session_id: Option<String>,
    parent_session_id: Option<String>,
    run_id: Option<String>,
}

#[derive(Default, Serialize)]
struct EmptyHookWorkspace {
    root: Option<String>,
}

#[derive(Default, Serialize)]
struct AppHookTruncation {
    truncated: bool,
    fields: BTreeSet<String>,
}

impl AppHookTruncation {
    fn record(&mut self, field: &str) {
        self.truncated = true;
        self.fields.insert(field.to_owned());
    }
}

pub(super) enum WorkflowPayload<'a> {
    Run {
        workflow_run_id: &'a str,
        plan_digest: &'a str,
        outcome: Option<&'a str>,
        duration_ms: Option<u64>,
        artifacts: &'a [crate::workflow::DurableArtifactReference],
    },
    Node {
        workflow_run_id: &'a str,
        plan_digest: &'a str,
        node_id: &'a str,
        attempt: u32,
        outcome: Option<&'a str>,
        duration_ms: Option<u64>,
        artifacts: &'a [crate::workflow::DurableArtifactReference],
    },
}

#[derive(Serialize)]
struct BoundedWorkflowPayload {
    workflow_run_id: String,
    plan_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    artifact_references: Vec<crate::workflow::DurableArtifactReference>,
}

impl BoundedWorkflowPayload {
    fn new(
        payload: WorkflowPayload<'_>,
        bounds: HookPayloadBounds,
        truncation: &mut AppHookTruncation,
    ) -> Self {
        let (workflow_run_id, plan_digest, node_id, attempt, outcome, duration_ms, artifacts) =
            match payload {
                WorkflowPayload::Run {
                    workflow_run_id,
                    plan_digest,
                    outcome,
                    duration_ms,
                    artifacts,
                } => (
                    workflow_run_id,
                    plan_digest,
                    None,
                    None,
                    outcome,
                    duration_ms,
                    artifacts,
                ),
                WorkflowPayload::Node {
                    workflow_run_id,
                    plan_digest,
                    node_id,
                    attempt,
                    outcome,
                    duration_ms,
                    artifacts,
                } => (
                    workflow_run_id,
                    plan_digest,
                    Some(node_id),
                    Some(attempt),
                    outcome,
                    duration_ms,
                    artifacts,
                ),
            };
        Self {
            workflow_run_id: bounded_app_string(
                workflow_run_id,
                "payload.workflow_run_id",
                bounds,
                truncation,
            ),
            plan_digest: bounded_app_string(plan_digest, "payload.plan_digest", bounds, truncation),
            node_id: node_id
                .map(|node_id| bounded_app_string(node_id, "payload.node_id", bounds, truncation)),
            attempt,
            outcome: outcome
                .map(|outcome| bounded_app_string(outcome, "payload.outcome", bounds, truncation)),
            duration_ms,
            artifact_references: bounded_artifacts(artifacts, bounds, truncation),
        }
    }
}

fn bounded_artifacts(
    artifacts: &[crate::workflow::DurableArtifactReference],
    bounds: HookPayloadBounds,
    truncation: &mut AppHookTruncation,
) -> Vec<crate::workflow::DurableArtifactReference> {
    let mut bounded = Vec::new();
    let mut encoded_bytes = 0usize;
    for (index, artifact) in artifacts.iter().enumerate() {
        let mut artifact = artifact.clone();
        artifact.artifact.relative_path = bounded_app_string(
            &artifact.artifact.relative_path,
            &format!("payload.artifact_references.{index}.relative_path"),
            bounds,
            truncation,
        );
        let item_bytes = serde_json::to_vec(&artifact)
            .map(|encoded| encoded.len().saturating_add(1))
            .unwrap_or(bounds.max_envelope_bytes());
        if encoded_bytes.saturating_add(item_bytes) > bounds.max_envelope_bytes() {
            truncation.record("payload.artifact_references");
            break;
        }
        encoded_bytes += item_bytes;
        bounded.push(artifact);
    }
    bounded
}

fn bounded_app_string(
    value: &str,
    field: &str,
    bounds: HookPayloadBounds,
    truncation: &mut AppHookTruncation,
) -> String {
    if value.len() <= bounds.max_field_bytes() {
        return value.to_owned();
    }
    let boundary = floor_char_boundary(value, bounds.max_field_bytes());
    truncation.record(field);
    value[..boundary].to_owned()
}

#[cfg(test)]
#[path = "workflow_event_tests.rs"]
mod tests;
