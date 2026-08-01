//! Plan/run inventory helpers for the durable workflow store.
//!
//! Hub and status UIs need name, lifecycle, and progress only. Full
//! `load_plan` / `load_run` rehash sources, re-digest graphs, and replay
//! event journals. Inventory reads the small on-disk fields the UI needs
//! and skips that validation path.
//!
//! This module is projection only. Destructive store mutations live on
//! [`WorkflowStore`] in `store.rs`.

use std::{collections::BTreeMap, path::Path, str::FromStr};

use serde::Deserialize;

use super::{plan_relative, read_json, run_relative, WorkflowStore};
use crate::workflow::{
    NodeId, NodeState, PlanId, PlanManifest, RunId, RunLifecycle, RunManifest, WorkflowOutcome,
    WorkflowResult,
};

/// Lightweight plan row for workspace inventory UIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlanInventoryItem {
    pub(crate) plan_id: PlanId,
    pub(crate) workspace_identity: String,
    pub(crate) name: String,
    pub(crate) step_count: usize,
}

/// Lightweight run row for workspace inventory UIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunInventoryItem {
    pub(crate) run_id: RunId,
    pub(crate) workspace_identity: String,
    pub(crate) name: String,
    pub(crate) lifecycle: RunLifecycle,
    pub(crate) outcome: Option<WorkflowOutcome>,
    pub(crate) done_steps: usize,
    pub(crate) total_steps: usize,
}

#[derive(Debug, Deserialize)]
struct InventoryGraphFile {
    graph: InventoryGraphBody,
}

#[derive(Debug, Deserialize)]
struct InventoryGraphBody {
    name: String,
    nodes: BTreeMap<String, serde::de::IgnoredAny>,
}

#[derive(Debug, Deserialize)]
struct InventoryStateFile {
    state: InventoryWorkflowState,
}

#[derive(Debug, Deserialize)]
struct InventoryWorkflowState {
    lifecycle: RunLifecycle,
    outcome: Option<WorkflowOutcome>,
    nodes: BTreeMap<NodeId, NodeState>,
}

#[derive(Debug, Deserialize)]
struct RevisionStateFile {
    state: RevisionOnly,
}

#[derive(Debug, Deserialize)]
struct RevisionOnly {
    revision: u64,
}

#[derive(Debug, Deserialize)]
struct LifecycleStateFile {
    state: LifecycleOnly,
}

#[derive(Debug, Deserialize)]
struct LifecycleOnly {
    lifecycle: RunLifecycle,
}

impl WorkflowStore {
    /// Lists plan rows without source rehash or graph re-digest.
    ///
    /// Non-UUID directory names are ignored. A valid plan ID directory that
    /// cannot be read fails the list; empty means empty, not broken.
    pub(crate) fn list_plan_inventory(&self) -> WorkflowResult<Vec<PlanInventoryItem>> {
        let mut plans = Vec::new();
        for name in self.root.directory_names(Path::new("plans"))? {
            let Ok(name) = name.into_string() else {
                continue;
            };
            let Ok(id) = PlanId::from_str(&name) else {
                continue;
            };
            plans.push(self.read_plan_inventory(id)?);
        }
        plans.sort_by_key(|plan| std::cmp::Reverse(plan.plan_id));
        Ok(plans)
    }

    /// Lists run rows without journal replay or frozen-graph validation.
    ///
    /// Non-UUID directory names (including trash names) are ignored. A valid
    /// run ID directory that cannot be read fails the list.
    pub(crate) fn list_run_inventory(&self) -> WorkflowResult<Vec<RunInventoryItem>> {
        let mut runs = Vec::new();
        for name in self.root.directory_names(Path::new("runs"))? {
            let Ok(name) = name.into_string() else {
                continue;
            };
            let Ok(id) = RunId::from_str(&name) else {
                continue;
            };
            runs.push(self.read_run_inventory(id)?);
        }
        runs.sort_by_key(|run| std::cmp::Reverse(run.run_id));
        Ok(runs)
    }

    /// Reads one plan manifest for workspace checks without loading the graph.
    pub(crate) fn read_plan_manifest(&self, id: PlanId) -> WorkflowResult<PlanManifest> {
        read_json(&self.root, &plan_relative(id, Path::new("manifest.json")))
    }

    /// Reads the durable revision without journal replay or graph validation.
    pub(crate) fn read_run_revision(&self, id: RunId) -> WorkflowResult<u64> {
        let state: RevisionStateFile =
            read_json(&self.root, &run_relative(id, Path::new("state.json")))?;
        Ok(state.state.revision)
    }

    /// Reads lifecycle without journal replay. Used by locked delete checks.
    pub(crate) fn read_run_lifecycle(&self, id: RunId) -> WorkflowResult<RunLifecycle> {
        let state: LifecycleStateFile =
            read_json(&self.root, &run_relative(id, Path::new("state.json")))?;
        Ok(state.state.lifecycle)
    }

    /// Reads one run inventory row without journal replay.
    pub(crate) fn read_run_inventory(&self, id: RunId) -> WorkflowResult<RunInventoryItem> {
        let manifest: RunManifest =
            read_json(&self.root, &run_relative(id, Path::new("manifest.json")))?;
        if manifest.run_id != id {
            return Err(crate::workflow::WorkflowError::Corrupt {
                path: self.layout.run_manifest(id),
                reason: "run manifest ID differs from its directory ID".to_owned(),
            });
        }
        let graph: InventoryGraphFile =
            read_json(&self.root, &run_relative(id, Path::new("graph.json")))?;
        let state: InventoryStateFile =
            read_json(&self.root, &run_relative(id, Path::new("state.json")))?;
        let total_steps = graph.graph.nodes.len();
        let done_steps = state
            .state
            .nodes
            .values()
            .filter(|node| node.terminal().is_some())
            .count();
        Ok(RunInventoryItem {
            run_id: id,
            workspace_identity: manifest.workspace_identity,
            name: graph.graph.name,
            lifecycle: state.state.lifecycle,
            outcome: state.state.outcome,
            done_steps,
            total_steps,
        })
    }

    fn read_plan_inventory(&self, id: PlanId) -> WorkflowResult<PlanInventoryItem> {
        let manifest: PlanManifest =
            read_json(&self.root, &plan_relative(id, Path::new("manifest.json")))?;
        if manifest.plan_id != id {
            return Err(crate::workflow::WorkflowError::Corrupt {
                path: self.layout.plan_manifest(id),
                reason: "plan manifest ID differs from its directory ID".to_owned(),
            });
        }
        let graph: InventoryGraphFile =
            read_json(&self.root, &plan_relative(id, Path::new("graph.json")))?;
        Ok(PlanInventoryItem {
            plan_id: id,
            workspace_identity: manifest.workspace_identity,
            name: graph.graph.name,
            step_count: graph.graph.nodes.len(),
        })
    }
}
