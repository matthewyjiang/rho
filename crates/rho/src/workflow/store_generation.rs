//! Manifest generation policy for the durable workflow store.
//!
//! Plans and runs saved before scoped workflow programs (Rho before 2.15) use
//! version 1 manifests. This release cannot open them: loads, writers, and
//! cancellation markers fail with [`WorkflowError::UnsupportedRecord`] naming
//! the record, so they never pass for missing or corrupt data. Inventory still
//! lists them from the manifest header so users can see and delete them.

use std::path::Path;

use serde::{
    de::{DeserializeOwned, IgnoredAny},
    Deserialize,
};

use super::{read_json, run_relative, WorkflowStore};
use crate::workflow::{
    check_schema_version, RunId, WorkflowError, WorkflowResult, PLAN_MANIFEST_VERSION,
    RUN_MANIFEST_VERSION,
};

/// Manifest version written before scoped workflow programs (Rho before 2.15).
pub(super) const UNSUPPORTED_MANIFEST_VERSION: u32 = 1;

/// Durable record kinds whose manifests carry a generation.
#[derive(Clone, Copy, Debug)]
pub(super) enum RecordKind {
    Plan,
    Run,
}

impl RecordKind {
    fn label(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Run => "run",
        }
    }

    fn manifest_label(self) -> &'static str {
        match self {
            Self::Plan => "plan manifest",
            Self::Run => "run manifest",
        }
    }

    fn id_key(self) -> &'static str {
        match self {
            Self::Plan => "plan_id",
            Self::Run => "run_id",
        }
    }

    fn current_version(self) -> u32 {
        match self {
            Self::Plan => PLAN_MANIFEST_VERSION,
            Self::Run => RUN_MANIFEST_VERSION,
        }
    }
}

impl WorkflowStore {
    /// Reads a manifest once, checks its directory ID and generation, then
    /// deserializes the current typed manifest.
    pub(super) fn read_manifest<T: DeserializeOwned>(
        &self,
        kind: RecordKind,
        relative: &Path,
        id: &str,
    ) -> WorkflowResult<T> {
        #[derive(Deserialize)]
        struct Version {
            schema_version: u32,
        }
        let value: serde_json::Value = read_json(&self.root, relative)?;
        let version = Version::deserialize(&value)?;
        if value.get(kind.id_key()).and_then(serde_json::Value::as_str) != Some(id) {
            return super::corrupt(
                &self.layout.root().join(relative),
                "manifest ID differs from its directory ID",
            );
        }
        if version.schema_version == UNSUPPORTED_MANIFEST_VERSION {
            return Err(WorkflowError::UnsupportedRecord {
                kind: kind.label(),
                id: id.to_owned(),
            });
        }
        check_schema_version(
            kind.manifest_label(),
            version.schema_version,
            kind.current_version(),
        )?;
        Ok(serde_json::from_value(value)?)
    }

    /// Refuses writers and cancellation markers on runs this release cannot open.
    pub(super) fn require_current_run(&self, id: RunId) -> WorkflowResult<()> {
        let _: IgnoredAny = self.read_manifest(
            RecordKind::Run,
            &run_relative(id, Path::new("manifest.json")),
            &id.to_string(),
        )?;
        Ok(())
    }
}
