use std::{
    fs::{self, File, OpenOptions},
    io::Write,
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
        if manifest.plan_id != id {
            return corrupt(
                &self.layout.plan_manifest(id),
                "plan manifest ID differs from its directory ID",
            );
        }
        let graph: FrozenWorkflow = read_json(&self.layout.plan_graph(id))?;
        check_schema_version(
            "frozen graph",
            graph.schema_version,
            FROZEN_WORKFLOW_SCHEMA_VERSION,
        )?;
        validate_plan(&self.layout, id, &manifest, &graph)?;
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
        validate_state(&plan.graph, &state, &[], &self.layout.runs())?;
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
        if manifest.run_id != id {
            return corrupt(
                &self.layout.run_manifest(id),
                "run manifest ID differs from its directory ID",
            );
        }
        let graph: FrozenWorkflow = read_json(&self.layout.run_graph(id))?;
        check_schema_version(
            "frozen graph",
            graph.schema_version,
            FROZEN_WORKFLOW_SCHEMA_VERSION,
        )?;
        validate_frozen_graph(&graph, &manifest.graph_digest, &self.layout.run_graph(id))?;
        validate_run_manifest(&manifest, &graph, &self.layout.run_manifest(id))?;
        let state: RunStateRecord = read_json(&self.layout.run_state(id))?;
        check_schema_version("run state", state.schema_version, RUN_STATE_VERSION)?;
        let events = self.read_events(id)?;
        validate_state(&graph, &state, &events, &self.layout.run_state(id))?;
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
        let journal = OpenOptions::new().read(true).write(true).open(&path)?;
        let bytes = fs::read(&path)?;
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
        let mut file = OpenOptions::new()
            .append(true)
            .open(self.layout.run_events(guard.id))?;
        serde_json::to_writer(&mut file, event)?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        guard.next_sequence = expected.saturating_add(1);
        Ok(())
    }

    pub(crate) fn save_state(
        &self,
        guard: &RunMutationGuard,
        state: &RunStateRecord,
    ) -> WorkflowResult<()> {
        check_schema_version("run state", state.schema_version, RUN_STATE_VERSION)?;
        let graph: FrozenWorkflow = read_json(&self.layout.run_graph(guard.id))?;
        let events = self.read_events(guard.id)?;
        validate_state(&graph, state, &events, &self.layout.run_state(guard.id))?;
        write_json(&self.layout.run_state(guard.id), state)
    }

    pub(crate) fn read_events(&self, id: RunId) -> WorkflowResult<Vec<WorkflowEventRecord>> {
        let path = self.layout.run_events(id);
        Ok(scan_journal(&path, &fs::read(&path)?)?.records)
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
    next_sequence: u64,
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
        reject_symlink(&path)?;
        let bytes = fs::read(&path)?;
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
            .is_none_or(|total| total > definition.max_output_bytes)
        {
            return corrupt(
                path,
                "command stream artifacts exceed the frozen total output limit",
            );
        }
        validate_completion_artifacts(completion, path)?;
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
            let mut file = super::secure_fs::open_file_beneath(run_directory, relative)?;
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
