//! Plan/run inventory helpers for the durable workflow store.

use std::{path::Path, str::FromStr};

use super::WorkflowStore;
use crate::workflow::{PlanId, RunId, StoredPlan, StoredRun, WorkflowError, WorkflowResult};

impl WorkflowStore {
    /// Lists stored plans. Skips unreadable entries so one corrupt plan does not hide the rest.
    pub(crate) fn list_plans(&self) -> WorkflowResult<Vec<StoredPlan>> {
        let mut plans = Vec::new();
        for name in self.root.directory_names(Path::new("plans"))? {
            let Ok(name) = name.into_string() else {
                continue;
            };
            let Ok(id) = PlanId::from_str(&name) else {
                continue;
            };
            if let Ok(plan) = self.load_plan(id) {
                plans.push(plan);
            }
        }
        plans.sort_by_key(|plan| std::cmp::Reverse(plan.manifest.plan_id));
        Ok(plans)
    }

    /// Lists stored runs. Skips unreadable entries so one corrupt run does not hide the rest.
    pub(crate) fn list_runs(&self) -> WorkflowResult<Vec<StoredRun>> {
        let mut runs = Vec::new();
        for name in self.root.directory_names(Path::new("runs"))? {
            let Ok(name) = name.into_string() else {
                continue;
            };
            let Ok(id) = RunId::from_str(&name) else {
                continue;
            };
            if let Ok(run) = self.load_run(id) {
                runs.push(run);
            }
        }
        runs.sort_by_key(|run| std::cmp::Reverse(run.manifest.run_id));
        Ok(runs)
    }

    /// Deletes one plan directory. Runs keep their copied graph, so resume still works.
    pub(crate) fn delete_plan(&self, id: PlanId) -> WorkflowResult<()> {
        // Confirm the plan is a real store entry before removal.
        let _ = self.load_plan(id)?;
        delete_child_directory(&self.layout.plans(), &self.layout.plan(id))
    }

    /// Deletes one run directory. Fails if another process holds the run lock.
    pub(crate) fn delete_run(&self, id: RunId) -> WorkflowResult<()> {
        let _ = self.load_run(id)?;
        // Refuse while an owner holds the exclusive writer lock.
        let guard = self.lock_run(id)?;
        drop(guard);
        delete_child_directory(&self.layout.runs(), &self.layout.run(id))
    }
}

/// Removes `child` only when it is a direct subdirectory of `parent`.
fn delete_child_directory(parent: &Path, child: &Path) -> WorkflowResult<()> {
    let parent = parent.canonicalize().map_err(WorkflowError::Io)?;
    let child = child.canonicalize().map_err(WorkflowError::Io)?;
    if child.parent() != Some(parent.as_path()) {
        return Err(WorkflowError::Corrupt {
            path: child,
            reason: "refusing to delete a path outside the store entry parent".to_owned(),
        });
    }
    std::fs::remove_dir_all(&child).map_err(WorkflowError::Io)?;
    Ok(())
}
