//! Destructive plan/run mutations for the durable workflow store.
//!
//! Inventory (`store_inventory.rs`) is projection only. Delete and path-guarded
//! removal live here next to the other durable mutation entry points.

use std::path::Path;

use fs2::FileExt;

use super::{delete_child_directory, read_json, run_relative, WorkflowStore};
use crate::workflow::{PlanId, RunId, RunManifest, WorkflowError, WorkflowResult};

impl WorkflowStore {
    /// Deletes one plan directory. Runs keep their copied graph, so resume still works.
    pub(crate) fn delete_plan(&self, id: PlanId) -> WorkflowResult<()> {
        // Confirm the plan directory is a real store entry before removal.
        let _ = self.read_plan_manifest(id)?;
        delete_child_directory(&self.layout.plans(), &self.layout.plan(id))
    }

    /// Deletes one run directory.
    ///
    /// Holds the exclusive writer lock across a rename of the run ID path so
    /// another process cannot `lock_run` and drive the tree while it is removed.
    /// Live (`Running` / `Cancelling`) runs are refused under that lock.
    pub(crate) fn delete_run(&self, id: RunId) -> WorkflowResult<()> {
        // Confirm the run directory is a real store entry before removal.
        let _: RunManifest = read_json(&self.root, &run_relative(id, Path::new("manifest.json")))?;
        let lock = self
            .root
            .open_private_file(&run_relative(id, Path::new("mutation.lock")), true)?;
        lock.try_lock_exclusive()
            .map_err(|error| WorkflowError::Corrupt {
                path: self.layout.run_lock(id),
                reason: format!("run already has an active writer: {error}"),
            })?;

        // Re-check lifecycle under the lock. Ops may have checked earlier, but
        // a concurrent owner could have advanced state before we took the lock.
        let lifecycle = self.read_run_lifecycle(id)?;
        if lifecycle.is_live() {
            let _ = lock.unlock();
            return Err(WorkflowError::Corrupt {
                path: self.layout.run(id),
                reason: format!(
                    "run is still {}, stop it before deleting",
                    format!("{lifecycle:?}").to_ascii_lowercase()
                ),
            });
        }

        // Rename while locked so the original ID path disappears before release.
        // Writers open mutation.lock by run ID path and cannot attach after this.
        let trash_name = format!(".trash-run-{id}-{}", uuid::Uuid::new_v4());
        let run_path = self.layout.run(id);
        let trash_path = self.layout.runs().join(&trash_name);
        if let Err(error) = std::fs::rename(&run_path, &trash_path) {
            let _ = lock.unlock();
            return Err(WorkflowError::Io(error));
        }
        let _ = lock.unlock();
        drop(lock);
        delete_child_directory(&self.layout.runs(), &trash_path)
    }
}
