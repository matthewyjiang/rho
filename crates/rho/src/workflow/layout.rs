use std::path::{Path, PathBuf};

use super::{PlanId, RunId};

#[derive(Clone, Debug)]
pub(crate) struct WorkflowLayout {
    root: PathBuf,
}

impl WorkflowLayout {
    pub(crate) fn new(rho_home: &Path) -> Self {
        Self {
            root: rho_home.join("workflows"),
        }
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn plans(&self) -> PathBuf {
        self.root.join("plans")
    }
    pub(crate) fn runs(&self) -> PathBuf {
        self.root.join("runs")
    }
    pub(crate) fn plan(&self, id: PlanId) -> PathBuf {
        self.plans().join(id.to_string())
    }
    pub(crate) fn run(&self, id: RunId) -> PathBuf {
        self.runs().join(id.to_string())
    }
    pub(crate) fn plan_manifest(&self, id: PlanId) -> PathBuf {
        self.plan(id).join("manifest.json")
    }
    pub(crate) fn plan_graph(&self, id: PlanId) -> PathBuf {
        self.plan(id).join("graph.json")
    }
    pub(crate) fn plan_sources(&self, id: PlanId) -> PathBuf {
        self.plan(id).join("sources")
    }
    pub(crate) fn run_manifest(&self, id: RunId) -> PathBuf {
        self.run(id).join("manifest.json")
    }
    pub(crate) fn run_graph(&self, id: RunId) -> PathBuf {
        self.run(id).join("graph.json")
    }
    pub(crate) fn run_state(&self, id: RunId) -> PathBuf {
        self.run(id).join("state.json")
    }
    pub(crate) fn run_events(&self, id: RunId) -> PathBuf {
        self.run(id).join("events.jsonl")
    }
    pub(crate) fn run_lock(&self, id: RunId) -> PathBuf {
        self.run(id).join("mutation.lock")
    }
}
