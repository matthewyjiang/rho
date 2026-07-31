use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use fs2::FileExt;
use serde::{de::DeserializeOwned, Serialize};

use super::{
    check_schema_version, FrozenWorkflow, PlanConsent, PlanId, PlanManifest, RunId, RunManifest,
    RunStateRecord, StoredPlan, StoredRun, WorkflowError, WorkflowEventRecord, WorkflowLayout,
    WorkflowResult, EVENT_VERSION, FROZEN_WORKFLOW_SCHEMA_VERSION, PLAN_MANIFEST_VERSION,
    RUN_MANIFEST_VERSION, RUN_STATE_VERSION,
};

pub(crate) struct WorkflowStore {
    layout: WorkflowLayout,
}

impl WorkflowStore {
    pub(crate) fn new(rho_home: &Path) -> WorkflowResult<Self> {
        let layout = WorkflowLayout::new(rho_home);
        ensure_private_dir(layout.root())?;
        ensure_private_dir(&layout.plans())?;
        ensure_private_dir(&layout.runs())?;
        Ok(Self { layout })
    }

    pub(crate) fn create_plan(
        &self,
        graph: &FrozenWorkflow,
        workspace_identity: String,
        source_bytes: &std::collections::BTreeMap<String, String>,
    ) -> WorkflowResult<StoredPlan> {
        verify_digest(graph, &graph.graph_digest)?;
        if graph.sources.modules.keys().ne(source_bytes.keys()) {
            return Err(WorkflowError::Corrupt {
                path: self.layout.plans(),
                reason: "source byte labels differ from the frozen source manifest".to_owned(),
            });
        }
        let mut unique_sources = std::collections::BTreeMap::new();
        for (label, source) in source_bytes {
            let digest = sha256(source.as_bytes());
            let expected = &graph.sources.modules[label];
            if expected.digest.0 != format!("sha256:{digest}")
                || expected.bytes != source.len() as u64
            {
                return Err(WorkflowError::Corrupt {
                    path: self.layout.plans(),
                    reason: format!("source bytes do not match manifest entry '{label}'"),
                });
            }
            unique_sources.entry(digest).or_insert(source);
        }
        let id = PlanId::new();
        let directory = self.layout.plan(id);
        create_private_dir(&directory)?;
        ensure_private_dir(&self.layout.plan_sources(id))?;
        let source_digests = graph
            .sources
            .modules
            .iter()
            .map(|(label, source)| (label.clone(), source.digest.clone()))
            .collect();
        let manifest = PlanManifest {
            schema_version: PLAN_MANIFEST_VERSION,
            plan_id: id,
            graph_digest: graph.graph_digest.clone(),
            workspace_identity,
            source_digests,
        };
        write_json(&self.layout.plan_manifest(id), &manifest)?;
        write_json(&self.layout.plan_graph(id), graph)?;
        for (digest, source) in unique_sources {
            write_new_private(
                &self.layout.plan_sources(id).join(format!("{digest}.star")),
                source.as_bytes(),
            )?;
        }
        Ok(StoredPlan {
            manifest,
            graph: graph.clone(),
        })
    }

    pub(crate) fn load_plan(&self, id: PlanId) -> WorkflowResult<StoredPlan> {
        let manifest: PlanManifest = read_json(&self.layout.plan_manifest(id))?;
        check_schema_version(
            "plan manifest",
            manifest.schema_version,
            PLAN_MANIFEST_VERSION,
        )?;
        let graph: FrozenWorkflow = read_json(&self.layout.plan_graph(id))?;
        check_schema_version(
            "frozen graph",
            graph.schema_version,
            FROZEN_WORKFLOW_SCHEMA_VERSION,
        )?;
        verify_digest(&graph, &manifest.graph_digest)?;
        Ok(StoredPlan { manifest, graph })
    }

    pub(crate) fn create_run(
        &self,
        plan: &StoredPlan,
        consent: PlanConsent,
        state: RunStateRecord,
    ) -> WorkflowResult<StoredRun> {
        if !consent.confirmed || consent.graph_digest != plan.manifest.graph_digest {
            return Err(WorkflowError::Corrupt {
                path: self.layout.plans(),
                reason: "run consent does not match the exact plan digest".to_owned(),
            });
        }
        let id = RunId::new();
        create_private_dir(&self.layout.run(id))?;
        let manifest = RunManifest {
            schema_version: RUN_MANIFEST_VERSION,
            run_id: id,
            plan_id: plan.manifest.plan_id,
            graph_digest: plan.manifest.graph_digest.clone(),
            workspace_identity: plan.manifest.workspace_identity.clone(),
            consent,
        };
        write_json(&self.layout.run_manifest(id), &manifest)?;
        write_json(&self.layout.run_graph(id), &plan.graph)?;
        write_json(&self.layout.run_state(id), &state)?;
        write_new_private(&self.layout.run_events(id), b"")?;
        write_new_private(&self.layout.run_lock(id), b"")?;
        Ok(StoredRun {
            manifest,
            graph: plan.graph.clone(),
            state,
        })
    }

    pub(crate) fn load_run(&self, id: RunId) -> WorkflowResult<StoredRun> {
        let manifest: RunManifest = read_json(&self.layout.run_manifest(id))?;
        check_schema_version(
            "run manifest",
            manifest.schema_version,
            RUN_MANIFEST_VERSION,
        )?;
        let graph: FrozenWorkflow = read_json(&self.layout.run_graph(id))?;
        check_schema_version(
            "frozen graph",
            graph.schema_version,
            FROZEN_WORKFLOW_SCHEMA_VERSION,
        )?;
        verify_digest(&graph, &manifest.graph_digest)?;
        let state: RunStateRecord = read_json(&self.layout.run_state(id))?;
        check_schema_version("run state", state.schema_version, RUN_STATE_VERSION)?;
        Ok(StoredRun {
            manifest,
            graph,
            state,
        })
    }

    pub(crate) fn lock_run(&self, id: RunId) -> WorkflowResult<RunMutationGuard> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.layout.run_lock(id))?;
        file.try_lock_exclusive()
            .map_err(|error| WorkflowError::Corrupt {
                path: self.layout.run_lock(id),
                reason: format!("run already has an active writer: {error}"),
            })?;
        let path = self.layout.run_events(id);
        let bytes = fs::read(&path)?;
        let next_sequence = if bytes.last().is_some_and(|byte| *byte != b'\n') {
            None
        } else {
            Some(
                self.read_events(id)?
                    .last()
                    .map_or(1, |record| record.sequence.saturating_add(1)),
            )
        };
        Ok(RunMutationGuard {
            id,
            next_sequence,
            file,
        })
    }

    pub(crate) fn append_event(
        &self,
        guard: &mut RunMutationGuard,
        event: &WorkflowEventRecord,
    ) -> WorkflowResult<()> {
        check_schema_version("workflow event", event.schema_version, EVENT_VERSION)?;
        let expected = guard.next_sequence.ok_or_else(|| WorkflowError::Corrupt {
            path: self.layout.run_events(guard.id),
            reason: "journal has a truncated tail and needs recovery before append".to_owned(),
        })?;
        if event.sequence != expected {
            return Err(WorkflowError::Corrupt {
                path: self.layout.run_events(guard.id),
                reason: format!(
                    "event sequence must be {expected}, requested {}",
                    event.sequence
                ),
            });
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(self.layout.run_events(guard.id))?;
        serde_json::to_writer(&mut file, event)?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        guard.next_sequence = Some(expected.saturating_add(1));
        Ok(())
    }

    pub(crate) fn save_state(
        &self,
        guard: &RunMutationGuard,
        state: &RunStateRecord,
    ) -> WorkflowResult<()> {
        check_schema_version("run state", state.schema_version, RUN_STATE_VERSION)?;
        write_json(&self.layout.run_state(guard.id), state)
    }

    pub(crate) fn read_events(&self, id: RunId) -> WorkflowResult<Vec<WorkflowEventRecord>> {
        let path = self.layout.run_events(id);
        let file = File::open(&path)?;
        let mut records = Vec::new();
        let mut lines = BufReader::new(file).split(b'\n').peekable();
        while let Some(line) = lines.next() {
            let line = line?;
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<WorkflowEventRecord>(&line) {
                Ok(record) => {
                    check_schema_version("workflow event", record.schema_version, EVENT_VERSION)?;
                    records.push(record);
                }
                Err(_) if lines.peek().is_none() => break,
                Err(error) => {
                    return Err(WorkflowError::Corrupt {
                        path,
                        reason: error.to_string(),
                    })
                }
            }
        }
        for pair in records.windows(2) {
            if pair[1].sequence != pair[0].sequence.saturating_add(1) {
                return Err(WorkflowError::Corrupt {
                    path: self.layout.run_events(id),
                    reason: "event sequence is not contiguous".to_owned(),
                });
            }
        }
        Ok(records)
    }

    pub(crate) fn resolve_plan(&self, prefix: &str) -> WorkflowResult<PlanId> {
        resolve_prefix(&self.layout.plans(), prefix)
    }

    pub(crate) fn resolve_run(&self, prefix: &str) -> WorkflowResult<RunId> {
        resolve_prefix(&self.layout.runs(), prefix)
    }
}

pub(crate) struct RunMutationGuard {
    id: RunId,
    next_sequence: Option<u64>,
    file: File,
}
impl Drop for RunMutationGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn verify_digest(graph: &FrozenWorkflow, expected: &super::Digest) -> WorkflowResult<()> {
    let actual = super::graph_digest(graph)?;
    if &actual == expected {
        Ok(())
    } else {
        Err(WorkflowError::Corrupt {
            path: PathBuf::from("graph.json"),
            reason: format!(
                "graph digest mismatch: expected {}, measured {}",
                expected.0, actual.0
            ),
        })
    }
}

fn resolve_prefix<T: FromStr<Err = WorkflowError>>(root: &Path, prefix: &str) -> WorkflowResult<T> {
    if prefix.is_empty() {
        return Err(WorkflowError::UnknownId(prefix.to_owned()));
    }
    let mut matches = fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (name.starts_with(prefix) && entry.file_type().ok()?.is_dir()).then_some(name)
        })
        .collect::<Vec<_>>();
    matches.sort();
    match matches.as_slice() {
        [value] => value.parse(),
        [] => Err(WorkflowError::UnknownId(prefix.to_owned())),
        values => Err(WorkflowError::AmbiguousId {
            prefix: prefix.to_owned(),
            matches: values.len(),
        }),
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> WorkflowResult<T> {
    reject_symlink(path)?;
    serde_json::from_slice(&fs::read(path)?).map_err(WorkflowError::from)
}
fn write_json(path: &Path, value: &impl Serialize) -> WorkflowResult<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    crate::config_writer::write_bytes_atomically(path, &bytes)?;
    set_private_file(path)?;
    Ok(())
}
fn ensure_private_dir(path: &Path) -> WorkflowResult<()> {
    if path.exists() {
        reject_symlink(path)?;
        if !path.is_dir() {
            return Err(WorkflowError::UntrustedDirectory(path.to_path_buf()));
        }
    } else {
        create_private_dir(path)?;
    }
    set_private_dir(path)?;
    Ok(())
}
fn create_private_dir(path: &Path) -> WorkflowResult<()> {
    fs::create_dir(path)?;
    set_private_dir(path)?;
    Ok(())
}
fn write_new_private(path: &Path, bytes: &[u8]) -> WorkflowResult<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn reject_symlink(path: &Path) -> WorkflowResult<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        Err(WorkflowError::UntrustedDirectory(path.to_path_buf()))
    } else {
        Ok(())
    }
}
#[cfg(unix)]
fn set_private_dir(path: &Path) -> WorkflowResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
#[cfg(not(unix))]
fn set_private_dir(_: &Path) -> WorkflowResult<()> {
    Ok(())
}
#[cfg(unix)]
fn set_private_file(path: &Path) -> WorkflowResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}
#[cfg(not(unix))]
fn set_private_file(_: &Path) -> WorkflowResult<()> {
    Ok(())
}
fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
