use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use fs2::FileExt;
use serde::{de::DeserializeOwned, Serialize};

use super::{
    check_schema_version, derive_workflow_outcome, validate_workflow, ArtifactObservation,
    FrozenWorkflow, NodeExecution, PlanConsent, PlanId, PlanManifest, RunId, RunLifecycle,
    RunManifest, RunStateRecord, StoredPlan, StoredRun, WorkflowError, WorkflowEventRecord,
    WorkflowLayout, WorkflowResult, EVENT_VERSION, FROZEN_WORKFLOW_SCHEMA_VERSION,
    PLAN_MANIFEST_VERSION, RUN_MANIFEST_VERSION, RUN_STATE_VERSION,
};

use super::store_replay::derive_snapshot;

pub(crate) struct WorkflowStore {
    layout: WorkflowLayout,
    root: super::secure_fs::SecureDirectory,
}

impl WorkflowStore {
    pub(crate) fn new(rho_home: &Path) -> WorkflowResult<Self> {
        let layout = WorkflowLayout::new(rho_home);
        if !rho_home.exists() {
            std::fs::create_dir_all(rho_home)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(rho_home, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        super::ensure_directory_beneath(rho_home, Path::new("workflows"))?;
        let root = super::secure_fs::SecureDirectory::open(layout.root())?;
        root.ensure_directory(Path::new("plans"))?;
        root.ensure_directory(Path::new("runs"))?;
        Ok(Self { layout, root })
    }

    pub(crate) fn create_plan(
        &self,
        graph: &FrozenWorkflow,
        workspace_identity: String,
        source_bytes: &std::collections::BTreeMap<String, String>,
    ) -> WorkflowResult<StoredPlan> {
        validate_frozen_graph(graph, &graph.graph_digest, &self.layout.plans())?;
        if workspace_identity.is_empty() {
            return corrupt(&self.layout.plans(), "plan workspace identity is empty");
        }
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
        self.root
            .ensure_directory(&plan_relative(id, Path::new("sources")))?;
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
            name: graph.graph.name.to_string(),
            step_count: graph.graph.nodes.len(),
        };
        write_json_beneath(
            &self.root,
            &plan_relative(id, Path::new("manifest.json")),
            &manifest,
        )?;
        write_json_beneath(
            &self.root,
            &plan_relative(id, Path::new("graph.json")),
            graph,
        )?;
        for (digest, source) in unique_sources {
            self.root.write_file(
                &plan_relative(id, &PathBuf::from("sources").join(format!("{digest}.star"))),
                source.as_bytes(),
            )?;
        }
        Ok(StoredPlan {
            manifest,
            graph: graph.clone(),
        })
    }

    pub(crate) fn load_plan(&self, id: PlanId) -> WorkflowResult<StoredPlan> {
        let manifest: PlanManifest =
            read_json(&self.root, &plan_relative(id, Path::new("manifest.json")))?;
        check_schema_version(
            "plan manifest",
            manifest.schema_version,
            PLAN_MANIFEST_VERSION,
        )?;
        if manifest.plan_id != id {
            return corrupt(
                &self.layout.plan_manifest(id),
                "plan manifest ID differs from its directory ID",
            );
        }
        let graph: FrozenWorkflow =
            read_json(&self.root, &plan_relative(id, Path::new("graph.json")))?;
        check_schema_version(
            "frozen graph",
            graph.schema_version,
            FROZEN_WORKFLOW_SCHEMA_VERSION,
        )?;
        validate_plan(&self.layout, &self.root, id, &manifest, &graph)?;
        Ok(StoredPlan { manifest, graph })
    }

    pub(crate) fn create_run(
        &self,
        plan: &StoredPlan,
        consent: PlanConsent,
        state: RunStateRecord,
    ) -> WorkflowResult<StoredRun> {
        validate_plan(
            &self.layout,
            &self.root,
            plan.manifest.plan_id,
            &plan.manifest,
            &plan.graph,
        )?;
        if !consent.confirmed || consent.graph_digest != plan.manifest.graph_digest {
            return Err(WorkflowError::Corrupt {
                path: self.layout.plans(),
                reason: "run consent does not match the exact plan digest".to_owned(),
            });
        }
        validate_state(
            &plan.graph,
            &state,
            &[],
            &self.layout.runs(),
            &self.root,
            Path::new("runs"),
        )?;
        let id = RunId::new();
        self.root
            .ensure_directory(&run_relative(id, Path::new("")))?;
        let manifest = RunManifest {
            schema_version: RUN_MANIFEST_VERSION,
            run_id: id,
            plan_id: plan.manifest.plan_id,
            graph_digest: plan.manifest.graph_digest.clone(),
            workspace_identity: plan.manifest.workspace_identity.clone(),
            consent,
            name: plan.graph.graph.name.to_string(),
            step_count: plan.graph.graph.nodes.len(),
        };
        write_json_beneath(
            &self.root,
            &run_relative(id, Path::new("manifest.json")),
            &manifest,
        )?;
        write_json_beneath(
            &self.root,
            &run_relative(id, Path::new("graph.json")),
            &plan.graph,
        )?;
        write_json_beneath(
            &self.root,
            &run_relative(id, Path::new("state.json")),
            &state,
        )?;
        self.root
            .write_file(&run_relative(id, Path::new("events.jsonl")), b"")?;
        self.root
            .write_file(&run_relative(id, Path::new("mutation.lock")), b"")?;
        Ok(StoredRun {
            manifest,
            graph: plan.graph.clone(),
            state,
        })
    }

    pub(crate) fn load_run(&self, id: RunId) -> WorkflowResult<StoredRun> {
        let manifest: RunManifest =
            read_json(&self.root, &run_relative(id, Path::new("manifest.json")))?;
        check_schema_version(
            "run manifest",
            manifest.schema_version,
            RUN_MANIFEST_VERSION,
        )?;
        if manifest.run_id != id {
            return corrupt(
                &self.layout.run_manifest(id),
                "run manifest ID differs from its directory ID",
            );
        }
        let graph: FrozenWorkflow =
            read_json(&self.root, &run_relative(id, Path::new("graph.json")))?;
        check_schema_version(
            "frozen graph",
            graph.schema_version,
            FROZEN_WORKFLOW_SCHEMA_VERSION,
        )?;
        validate_frozen_graph(&graph, &manifest.graph_digest, &self.layout.run_graph(id))?;
        validate_run_manifest(&manifest, &graph, &self.layout.run_manifest(id))?;
        let state: RunStateRecord =
            read_json(&self.root, &run_relative(id, Path::new("state.json")))?;
        check_schema_version("run state", state.schema_version, RUN_STATE_VERSION)?;
        let events = self.read_events(id)?;
        validate_state(
            &graph,
            &state,
            &events,
            &self.layout.run_state(id),
            &self.root,
            &run_relative(id, Path::new("")),
        )?;
        Ok(StoredRun {
            manifest,
            graph,
            state,
        })
    }

    pub(crate) fn lock_run(&self, id: RunId) -> WorkflowResult<RunMutationGuard> {
        let file = self
            .root
            .open_private_file(&run_relative(id, Path::new("mutation.lock")), true)?;
        file.try_lock_exclusive()
            .map_err(|error| WorkflowError::Corrupt {
                path: self.layout.run_lock(id),
                reason: format!("run already has an active writer: {error}"),
            })?;
        let path = self.layout.run_events(id);
        let mut journal = self
            .root
            .open_private_file(&run_relative(id, Path::new("events.jsonl")), true)?;
        let mut bytes = Vec::new();
        journal.read_to_end(&mut bytes)?;
        let scan = scan_journal(&path, &bytes)?;
        if scan.valid_bytes != bytes.len() {
            journal.set_len(scan.valid_bytes as u64)?;
            journal.sync_all()?;
        }
        let next_sequence = scan
            .records
            .last()
            .map_or(1, |record| record.sequence.saturating_add(1));
        Ok(RunMutationGuard {
            id,
            next_sequence,
            file,
            journal,
        })
    }

    pub(crate) fn append_event(
        &self,
        guard: &mut RunMutationGuard,
        event: &WorkflowEventRecord,
    ) -> WorkflowResult<()> {
        check_schema_version("workflow event", event.schema_version, EVENT_VERSION)?;
        let expected = guard.next_sequence;
        if event.sequence != expected {
            return Err(WorkflowError::Corrupt {
                path: self.layout.run_events(guard.id),
                reason: format!(
                    "event sequence must be {expected}, requested {}",
                    event.sequence
                ),
            });
        }
        guard.journal.seek(SeekFrom::End(0))?;
        serde_json::to_writer(&mut guard.journal, event)?;
        guard.journal.write_all(b"\n")?;
        guard.journal.sync_data()?;
        guard.next_sequence = expected.saturating_add(1);
        Ok(())
    }

    pub(crate) fn save_state(
        &self,
        guard: &RunMutationGuard,
        state: &RunStateRecord,
    ) -> WorkflowResult<()> {
        check_schema_version("run state", state.schema_version, RUN_STATE_VERSION)?;
        let graph: FrozenWorkflow =
            read_json(&self.root, &run_relative(guard.id, Path::new("graph.json")))?;
        let events = self.read_events(guard.id)?;
        validate_state(
            &graph,
            state,
            &events,
            &self.layout.run_state(guard.id),
            &self.root,
            &run_relative(guard.id, Path::new("")),
        )?;
        write_json_beneath(
            &self.root,
            &run_relative(guard.id, Path::new("state.json")),
            state,
        )
    }

    pub(crate) fn read_events(&self, id: RunId) -> WorkflowResult<Vec<WorkflowEventRecord>> {
        let path = self.layout.run_events(id);
        let mut file = self
            .root
            .open_private_file(&run_relative(id, Path::new("events.jsonl")), false)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(scan_journal(&path, &bytes)?.records)
    }

    pub(crate) fn read_cancellation_request(&self, id: RunId) -> WorkflowResult<Option<Vec<u8>>> {
        let relative = run_relative(id, Path::new("cancel.request"));
        let mut file = match self.root.open_private_file(&relative, false) {
            Ok(file) => file,
            Err(WorkflowError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        Read::by_ref(&mut file).take(37).read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    pub(crate) fn install_cancellation_request(
        &self,
        id: RunId,
        bytes: &[u8],
    ) -> WorkflowResult<bool> {
        self.root
            .write_file_if_absent(&run_relative(id, Path::new("cancel.request")), bytes)
    }

    pub(crate) fn clear_cancellation_request(&self, id: RunId) -> WorkflowResult<()> {
        match self
            .root
            .remove_file(&run_relative(id, Path::new("cancel.request")))
        {
            Ok(()) => Ok(()),
            Err(WorkflowError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn resolve_plan(&self, prefix: &str) -> WorkflowResult<PlanId> {
        resolve_prefix(&self.root, Path::new("plans"), prefix)
    }

    pub(crate) fn resolve_run(&self, prefix: &str) -> WorkflowResult<RunId> {
        resolve_prefix(&self.root, Path::new("runs"), prefix)
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

#[path = "store_inventory.rs"]
mod inventory;
pub(crate) use inventory::{PlanInventoryItem, RunInventoryItem};

#[path = "store_mutate.rs"]
mod mutate;

pub(crate) struct RunMutationGuard {
    id: RunId,
    next_sequence: u64,
    file: File,
    journal: File,
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

fn validate_frozen_graph(
    graph: &FrozenWorkflow,
    expected_digest: &super::Digest,
    path: &Path,
) -> WorkflowResult<()> {
    check_schema_version(
        "frozen graph",
        graph.schema_version,
        FROZEN_WORKFLOW_SCHEMA_VERSION,
    )?;
    if &graph.graph_digest != expected_digest {
        return corrupt(
            path,
            "frozen graph self-digest differs from its manifest digest",
        );
    }
    verify_digest(graph, expected_digest)?;
    validate_workflow(graph).map_err(|error| WorkflowError::Corrupt {
        path: path.to_path_buf(),
        reason: format!("frozen graph validation failed: {error}"),
    })?;
    let retained_bound = graph.graph.nodes.values().try_fold(0_u64, |total, node| {
        let streams = if matches!(node.execution, NodeExecution::Command(_)) {
            2
        } else {
            1
        };
        total
            .checked_add(node.max_output_bytes.checked_mul(streams).ok_or_else(|| {
                WorkflowError::Corrupt {
                    path: path.to_path_buf(),
                    reason: "frozen retained output bound overflowed".to_owned(),
                }
            })?)
            .ok_or_else(|| WorkflowError::Corrupt {
                path: path.to_path_buf(),
                reason: "frozen retained output bound overflowed".to_owned(),
            })
    })?;
    if retained_bound > graph.runtime_limits.retained_output_total_bytes {
        return corrupt(path, "frozen graph exceeds its workflow-wide output limit");
    }
    if !graph
        .sources
        .modules
        .contains_key(&graph.sources.entry_label)
    {
        return corrupt(
            path,
            "source entry label is absent from the source manifest",
        );
    }
    Ok(())
}

fn validate_plan(
    layout: &WorkflowLayout,
    root: &super::secure_fs::SecureDirectory,
    id: PlanId,
    manifest: &PlanManifest,
    graph: &FrozenWorkflow,
) -> WorkflowResult<()> {
    if manifest.plan_id != id {
        return corrupt(
            &layout.plan_manifest(id),
            "plan manifest ID differs from its directory ID",
        );
    }
    if manifest.workspace_identity.is_empty() {
        return corrupt(
            &layout.plan_manifest(id),
            "plan workspace identity is empty",
        );
    }
    validate_frozen_graph(graph, &manifest.graph_digest, &layout.plan_graph(id))?;
    let graph_digests = graph
        .sources
        .modules
        .iter()
        .map(|(label, source)| (label.clone(), source.digest.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    if manifest.source_digests != graph_digests {
        return corrupt(
            &layout.plan_manifest(id),
            "plan source digests differ from the frozen source manifest",
        );
    }
    for (label, source) in &graph.sources.modules {
        let digest =
            source
                .digest
                .0
                .strip_prefix("sha256:")
                .ok_or_else(|| WorkflowError::Corrupt {
                    path: layout.plan_manifest(id),
                    reason: format!("source '{label}' has an unsupported digest"),
                })?;
        let path = layout.plan_sources(id).join(format!("{digest}.star"));
        let mut file = root.open_private_file(
            &plan_relative(id, &PathBuf::from("sources").join(format!("{digest}.star"))),
            false,
        )?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        if bytes.len() as u64 != source.bytes || sha256(&bytes) != digest {
            return corrupt(
                &path,
                &format!("source blob for '{label}' does not match its metadata"),
            );
        }
    }
    Ok(())
}

fn validate_run_manifest(
    manifest: &RunManifest,
    graph: &FrozenWorkflow,
    path: &Path,
) -> WorkflowResult<()> {
    if manifest.workspace_identity.is_empty() {
        return corrupt(path, "run workspace identity is empty");
    }
    if !manifest.consent.confirmed
        || manifest.consent.graph_digest != manifest.graph_digest
        || manifest.graph_digest != graph.graph_digest
    {
        return corrupt(path, "run consent and graph digest do not match");
    }
    Ok(())
}

fn validate_state(
    graph: &FrozenWorkflow,
    record: &RunStateRecord,
    events: &[WorkflowEventRecord],
    path: &Path,
    root: &super::secure_fs::SecureDirectory,
    run_relative: &Path,
) -> WorkflowResult<()> {
    check_schema_version("run state", record.schema_version, RUN_STATE_VERSION)?;
    let state = &record.state;
    if graph.graph.nodes.keys().ne(state.nodes.keys()) {
        return corrupt(path, "node state keys differ from frozen graph keys");
    }
    let tail = events.last().map_or(0, |event| event.sequence);
    if record.last_event_sequence > tail {
        return corrupt(path, "snapshot sequence is ahead of the journal tail");
    }
    let derived =
        derive_snapshot(graph, events, record.last_event_sequence, path).map_err(|error| {
            match error {
                WorkflowError::Corrupt { .. } => error,
                error => WorkflowError::Corrupt {
                    path: path.to_path_buf(),
                    reason: format!("journal prefix does not derive valid state: {error}"),
                },
            }
        })?;
    if record.state != derived {
        return corrupt(path, "snapshot state differs from its journal prefix");
    }
    let mut retained_workflow_output = 0_u64;
    let terminal = state
        .nodes
        .iter()
        .filter_map(|(node, state)| state.terminal().map(|outcome| (node, outcome)))
        .collect::<std::collections::BTreeMap<_, _>>();
    if terminal.keys().copied().ne(state.completions.keys()) {
        return corrupt(path, "completion keys differ from terminal node keys");
    }
    for (node, completion) in &state.completions {
        if terminal.get(node).copied() != Some(completion.outcome) {
            return corrupt(path, "completion outcome differs from terminal node state");
        }
        if (completion.outcome == super::NodeTerminalState::Cancellation)
            != completion.cancellation_resume.is_some()
        {
            return corrupt(path, "completion has an invalid cancellation resume state");
        }
        if completion.attempt.is_none()
            && (completion.command_exit.is_some()
                || completion.structured_output.is_some()
                || completion.artifacts.iter().next().is_some())
        {
            return corrupt(path, "synthetic completion contains attempt-owned data");
        }
        let definition = &graph.graph.nodes[node];
        match &definition.execution {
            NodeExecution::Agent(_)
                if completion.command_exit.is_some()
                    || completion.artifacts.stdout.is_some()
                    || completion.artifacts.stderr.is_some()
                    || completion.artifacts.command_outcome.is_some() =>
            {
                return corrupt(path, "agent completion contains command artifacts")
            }
            NodeExecution::Command(_) if completion.artifacts.answer.is_some() => {
                return corrupt(path, "command completion contains an agent answer")
            }
            NodeExecution::Command(_)
                if completion.command_exit.is_some()
                    && (completion.artifacts.stdout.is_none()
                        || completion.artifacts.stderr.is_none()
                        || completion.artifacts.command_outcome.is_none()) =>
            {
                return corrupt(path, "command exit is missing durable command artifacts")
            }
            NodeExecution::Agent(_) | NodeExecution::Command(_) => {}
        }
        if let Some(output) = &completion.structured_output {
            let schema = definition
                .output_schema()
                .ok_or_else(|| WorkflowError::Corrupt {
                    path: path.to_path_buf(),
                    reason: format!("node '{node}' has output without a frozen schema"),
                })?;
            schema
                .validate_value(&output.value)
                .map_err(|error| WorkflowError::Corrupt {
                    path: path.to_path_buf(),
                    reason: format!("node '{node}' has invalid durable output: {error}"),
                })?;
        }
        for artifact in [
            completion.artifacts.stdout.as_ref(),
            completion.artifacts.stderr.as_ref(),
            completion.artifacts.answer.as_ref(),
            completion.artifacts.structured_output.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if artifact.retained_bytes > definition.max_output_bytes {
                return corrupt(
                    path,
                    "completion artifact exceeds its frozen node output limit",
                );
            }
        }
        if completion
            .artifacts
            .stdout
            .iter()
            .chain(completion.artifacts.stderr.iter())
            .try_fold(0_u64, |total, artifact| {
                total.checked_add(artifact.retained_bytes)
            })
            .is_none_or(|total| total > definition.max_output_bytes.saturating_mul(2))
        {
            return corrupt(
                path,
                "command stream artifacts exceed twice the per-stream output limit",
            );
        }
        retained_workflow_output = [
            completion.artifacts.stdout.as_ref(),
            completion.artifacts.stderr.as_ref(),
            completion.artifacts.answer.as_ref(),
        ]
        .into_iter()
        .flatten()
        .try_fold(retained_workflow_output, |total, artifact| {
            total.checked_add(artifact.retained_bytes)
        })
        .ok_or_else(|| WorkflowError::Corrupt {
            path: path.to_path_buf(),
            reason: "retained workflow output size overflowed".to_owned(),
        })?;
        validate_completion_artifacts(completion, path, root, run_relative)?;
    }
    if retained_workflow_output > graph.runtime_limits.retained_output_total_bytes {
        return corrupt(
            path,
            "retained output exceeds the frozen workflow-wide limit",
        );
    }
    let expected_exits = state
        .completions
        .iter()
        .filter_map(|(node, completion)| {
            completion
                .command_exit
                .clone()
                .map(|exit| (node.clone(), exit))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let expected_outputs = state
        .completions
        .iter()
        .filter_map(|(node, completion)| {
            completion
                .structured_output
                .as_ref()
                .map(|output| (node.clone(), output.value.clone()))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    if state.command_exits != expected_exits || state.outputs != expected_outputs {
        return corrupt(
            path,
            "output or command-exit keys differ from durable completions",
        );
    }
    match state.lifecycle {
        RunLifecycle::Completed => {
            if terminal.len() != graph.graph.nodes.len() {
                return corrupt(path, "completed run contains non-terminal nodes");
            }
            if state.outcome != derive_workflow_outcome(graph, state) {
                return corrupt(path, "completed run outcome is absent or not derived");
            }
        }
        _ if state.outcome.is_some() => {
            return corrupt(path, "non-completed run contains a workflow outcome")
        }
        _ => {}
    }
    Ok(())
}

fn validate_completion_artifacts(
    completion: &super::NodeCompletion,
    path: &Path,
    root: &super::secure_fs::SecureDirectory,
    run_relative: &Path,
) -> WorkflowResult<()> {
    if completion
        .structured_output
        .as_ref()
        .map(|output| &output.artifact)
        != completion.artifacts.structured_output.as_ref()
    {
        return corrupt(
            path,
            "structured output and completion artifact references differ",
        );
    }
    let artifacts = [
        completion.artifacts.stdout.as_ref(),
        completion.artifacts.stderr.as_ref(),
        completion.artifacts.answer.as_ref(),
        completion.artifacts.structured_output.as_ref(),
        completion.artifacts.command_outcome.as_ref(),
    ];
    for artifact in artifacts.into_iter().flatten() {
        let observation_valid = match artifact.observed {
            ArtifactObservation::Complete { observed_bytes } => {
                observed_bytes == artifact.retained_bytes
            }
            ArtifactObservation::Truncated {
                observed_bytes_at_least,
            } => observed_bytes_at_least > artifact.retained_bytes,
            ArtifactObservation::Incomplete { observed_bytes } => {
                observed_bytes >= artifact.retained_bytes
            }
        };
        if artifact.relative_path.is_empty() || !observation_valid {
            return corrupt(path, "durable artifact reference has invalid size metadata");
        }
        let relative = Path::new(&artifact.relative_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return corrupt(path, "durable artifact reference leaves the run directory");
        }
        if let Some(run_directory) = path.parent() {
            let artifact_path = run_directory.join(relative);
            let mut file = root.open_file(&run_relative.join(relative))?;
            let metadata = file.metadata().map_err(WorkflowError::Io)?;
            if metadata.len() != artifact.retained_bytes {
                return corrupt(
                    &artifact_path,
                    "durable artifact size does not match its file",
                );
            }
            use sha2::Digest as _;
            let mut hasher = sha2::Sha256::new();
            std::io::copy(&mut file, &mut hasher).map_err(WorkflowError::Io)?;
            let digest = format!("sha256:{:x}", hasher.finalize());
            if artifact.digest.0 != digest {
                return corrupt(
                    &artifact_path,
                    "durable artifact reference does not match its file",
                );
            }
        }
    }
    Ok(())
}

struct JournalScan {
    records: Vec<WorkflowEventRecord>,
    valid_bytes: usize,
}

fn scan_journal(path: &Path, bytes: &[u8]) -> WorkflowResult<JournalScan> {
    let valid_bytes = if bytes.is_empty() || bytes.ends_with(b"\n") {
        bytes.len()
    } else {
        bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |position| position + 1)
    };
    let mut records = Vec::new();
    let prefix = &bytes[..valid_bytes];
    let records_bytes = prefix.strip_suffix(b"\n").unwrap_or(prefix);
    for line in records_bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !records_bytes.is_empty() || !line.is_empty())
    {
        if line.is_empty() {
            return corrupt(path, "journal contains an empty record");
        }
        let record: WorkflowEventRecord =
            serde_json::from_slice(line).map_err(|error| WorkflowError::Corrupt {
                path: path.to_path_buf(),
                reason: format!("invalid journal prefix: {error}"),
            })?;
        check_schema_version("workflow event", record.schema_version, EVENT_VERSION)?;
        records.push(record);
    }
    if let Some(first) = records.first() {
        if first.sequence != 1 {
            return corrupt(path, "first journal sequence is not 1");
        }
    }
    for pair in records.windows(2) {
        if pair[1].sequence != pair[0].sequence.saturating_add(1) {
            return corrupt(path, "event sequence is not contiguous");
        }
    }
    let mut cancellation_requests = std::collections::BTreeSet::new();
    let mut cancellation_acknowledgements = std::collections::BTreeSet::new();
    for record in &records {
        match &record.event {
            super::WorkflowEvent::CancellationRequested { request_id } => {
                validate_cancellation_request_id(path, request_id)?;
                if !cancellation_requests.insert(request_id.clone()) {
                    return corrupt(path, "duplicate cancellation request identifier");
                }
            }
            super::WorkflowEvent::CancellationAcknowledged { request_id } => {
                validate_cancellation_request_id(path, request_id)?;
                if !cancellation_requests.contains(request_id) {
                    return corrupt(path, "cancellation acknowledgement has no request");
                }
                if !cancellation_acknowledgements.insert(request_id.clone()) {
                    return corrupt(path, "duplicate cancellation acknowledgement");
                }
            }
            _ => {}
        }
    }
    Ok(JournalScan {
        records,
        valid_bytes,
    })
}

fn validate_cancellation_request_id(path: &Path, request_id: &str) -> WorkflowResult<()> {
    let request = uuid::Uuid::parse_str(request_id).map_err(|_| WorkflowError::Corrupt {
        path: path.to_path_buf(),
        reason: "cancellation event has an invalid request identifier".into(),
    })?;
    if request.to_string() != request_id {
        return corrupt(
            path,
            "cancellation event request identifier is not canonical",
        );
    }
    Ok(())
}

fn corrupt<T>(path: &Path, reason: &str) -> WorkflowResult<T> {
    Err(WorkflowError::Corrupt {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    })
}

fn resolve_prefix<T: FromStr<Err = WorkflowError>>(
    root: &super::secure_fs::SecureDirectory,
    directory: &Path,
    prefix: &str,
) -> WorkflowResult<T> {
    if prefix.is_empty() {
        return Err(WorkflowError::UnknownId(prefix.to_owned()));
    }
    let mut matches = root
        .directory_names(directory)?
        .into_iter()
        .filter_map(|name| {
            let name = name.into_string().ok()?;
            (name.starts_with(prefix) && root.open_directory(&directory.join(&name)).is_ok())
                .then_some(name)
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

fn read_json<T: DeserializeOwned>(
    root: &super::secure_fs::SecureDirectory,
    relative: &Path,
) -> WorkflowResult<T> {
    let mut file = root.open_private_file(relative, false)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(WorkflowError::from)
}
fn write_json_beneath(
    root: &super::secure_fs::SecureDirectory,
    relative: &Path,
    value: &impl Serialize,
) -> WorkflowResult<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    root.write_file(relative, &bytes)
}

#[cfg(test)]
fn write_json(path: &Path, value: &impl Serialize) -> WorkflowResult<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let parent = path
        .parent()
        .ok_or_else(|| WorkflowError::UntrustedDirectory(path.to_path_buf()))?;
    let name = path
        .file_name()
        .ok_or_else(|| WorkflowError::UntrustedDirectory(path.to_path_buf()))?;
    super::write_file_beneath(parent, Path::new(name), &bytes)
}

fn plan_relative(id: PlanId, child: &Path) -> PathBuf {
    PathBuf::from("plans").join(id.to_string()).join(child)
}

fn run_relative(id: RunId, child: &Path) -> PathBuf {
    PathBuf::from("runs").join(id.to_string()).join(child)
}
fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
