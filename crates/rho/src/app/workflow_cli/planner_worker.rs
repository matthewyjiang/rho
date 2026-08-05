//! Supervised workflow planning process and its framed channel.

use super::*;

pub(crate) fn planning_limits() -> WorkflowResult<PlanningLimits> {
    PlanningLimits::from_measurements(planning_measurements())
}

#[derive(Deserialize)]
struct WorkflowLimitReceipt {
    cancellation: CancellationLimitReceipt,
    planning: PlanningLimitReceipt,
}

#[derive(Deserialize)]
struct CancellationLimitReceipt {
    accepted_acknowledgement_millis: u64,
    poll_millis: u64,
}

#[derive(Deserialize)]
struct PlanningLimitReceipt {
    accepted: PlanningMeasurements,
}

#[derive(Serialize, Deserialize)]
struct PlannerWorkerRequest {
    token: String,
    entry_label: String,
    sources: BTreeMap<String, String>,
    manifest: SourceManifest,
    inputs: BTreeMap<InputName, WorkflowValue>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct PlannerWorkerPlan {
    pub(super) graph: crate::workflow::WorkflowGraph,
    pub(super) inputs: BTreeMap<InputName, WorkflowValue>,
    pub(super) evaluator_ticks: u64,
    pub(super) evaluator_peak_heap_bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct PlannerWorkerResponse {
    plan: Option<PlannerWorkerPlan>,
    error: Option<String>,
}

fn planning_measurements() -> PlanningMeasurements {
    workflow_limit_receipt().planning.accepted
}

pub(super) fn cancellation_acknowledgement_limit_millis() -> u64 {
    workflow_limit_receipt()
        .cancellation
        .accepted_acknowledgement_millis
}

pub(super) fn cancellation_acknowledgement_poll_millis() -> u64 {
    workflow_limit_receipt().cancellation.poll_millis
}

fn workflow_limit_receipt() -> WorkflowLimitReceipt {
    serde_json::from_str(include_str!("../../workflow/fixtures/limit_receipt.json"))
        .expect("checked-in workflow limit receipt must match its schema")
}

pub(super) async fn run_supervised_planner(
    sources: &CollectedSources,
    inputs: BTreeMap<InputName, WorkflowValue>,
    limits: &PlanningLimits,
) -> anyhow::Result<PlannerWorkerPlan> {
    let request = PlannerWorkerRequest {
        token: planner_token(),
        entry_label: sources.entry_label.clone(),
        sources: sources.sources.clone(),
        manifest: sources.manifest.clone(),
        inputs,
    };
    let bytes = serde_json::to_vec(&request)?;
    check_frame_size(
        "planning worker request frame bytes",
        PLANNER_REQUEST_FRAME_BYTES,
        bytes.len() as u64,
    )?;
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .args([crate::cli::WORKFLOW_PLANNER_WORKER_COMMAND])
        .env_clear()
        .env(PLANNER_WORKER_ENV, &request.token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().expect("planner worker stdin is piped");
    write_frame_async(&mut stdin, &bytes).await?;
    stdin.shutdown().await?;
    drop(stdin);
    let stdout = child.stdout.take().expect("planner worker stdout is piped");
    let stderr = child.stderr.take().expect("planner worker stderr is piped");
    let completed = tokio::time::timeout(
        Duration::from_millis(limits.worker_wall_millis.limit),
        async move {
            let response = read_frame_async(stdout, PLANNER_RESPONSE_FRAME_BYTES);
            let diagnostics = async move {
                read_retained(stderr, PLANNER_STDERR_BYTES)
                    .await
                    .map_err(anyhow::Error::from)
            };
            let status = async move { child.wait().await.map_err(anyhow::Error::from) };
            Ok::<_, anyhow::Error>(tokio::join!(response, diagnostics, status))
        },
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "{} budget exceeded: planning worker did not finish within {} ms",
            limits.worker_wall_millis.name,
            limits.worker_wall_millis.limit
        )
    })??;
    let (response_bytes, diagnostics, status) = completed;
    let diagnostics = diagnostics?;
    let status = status?;
    if !status.success() {
        let diagnostics = String::from_utf8_lossy(&diagnostics);
        let diagnostics = diagnostics.trim();
        if diagnostics.is_empty() {
            anyhow::bail!("workflow planner worker failed with {status} and no stderr");
        }
        anyhow::bail!("workflow planner worker failed: {diagnostics}");
    }
    let response_bytes = response_bytes?;
    let response: PlannerWorkerResponse = serde_json::from_slice(&response_bytes)?;
    match (response.plan, response.error) {
        (Some(plan), None) => Ok(plan),
        (None, Some(error)) => anyhow::bail!(error),
        _ => anyhow::bail!("workflow planner worker returned an invalid response"),
    }
}

pub(crate) async fn run_planner_worker() -> anyhow::Result<()> {
    apply_planner_worker_limits()?;
    let expected_token = std::env::var(PLANNER_WORKER_ENV)
        .map_err(|_| anyhow::anyhow!("planner worker channel token is missing"))?;
    if !valid_planner_token(&expected_token) {
        anyhow::bail!("planner worker channel token is invalid");
    }
    let bytes = read_frame_sync(io::stdin().lock(), PLANNER_REQUEST_FRAME_BYTES)?;
    let request: PlannerWorkerRequest = serde_json::from_slice(&bytes)?;
    if !constant_time_eq(request.token.as_bytes(), expected_token.as_bytes()) {
        anyhow::bail!("planner worker channel authentication failed");
    }
    let limits = planning_limits()?;
    let collected = CollectedSources {
        entry_label: request.entry_label,
        sources: request.sources,
        manifest: request.manifest,
    };
    collected.validate(&limits)?;
    let response = match StarlarkPlanner::new(&limits).plan_in_process_prototype(
        &collected,
        &request.inputs,
        Arc::new(AtomicBool::new(false)),
    ) {
        Ok(planned) => PlannerWorkerResponse {
            plan: Some(PlannerWorkerPlan {
                graph: planned.graph,
                inputs: planned.inputs,
                evaluator_ticks: planned.ticks,
                evaluator_peak_heap_bytes: planned.peak_heap_bytes,
            }),
            error: None,
        },
        Err(error) => PlannerWorkerResponse {
            plan: None,
            error: Some(error.to_string()),
        },
    };
    let response = serde_json::to_vec(&response)?;
    check_frame_size(
        "planning worker response frame bytes",
        PLANNER_RESPONSE_FRAME_BYTES,
        response.len() as u64,
    )?;
    write_frame_sync(io::stdout().lock(), &response)?;
    Ok(())
}

fn planner_token() -> String {
    use rand::RngCore;
    let mut bytes = [0_u8; PLANNER_TOKEN_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn valid_planner_token(token: &str) -> bool {
    token.len() == PLANNER_TOKEN_BYTES * 2 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn check_frame_size(name: &'static str, limit: u64, actual: u64) -> WorkflowResult<()> {
    if actual == 0 || actual > limit {
        return Err(WorkflowError::BudgetExceeded {
            budget: name,
            limit,
            actual,
        });
    }
    Ok(())
}

async fn write_frame_async(output: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> io::Result<()> {
    output
        .write_all(&(bytes.len() as u64).to_be_bytes())
        .await?;
    output.write_all(bytes).await
}

async fn read_frame_async(
    mut input: impl AsyncRead + Unpin,
    limit: u64,
) -> anyhow::Result<Vec<u8>> {
    let mut header = [0_u8; 8];
    input.read_exact(&mut header).await?;
    let length = u64::from_be_bytes(header);
    check_frame_size("planning IPC frame bytes", limit, length)?;
    let mut bytes = vec![0_u8; usize::try_from(length)?];
    input.read_exact(&mut bytes).await?;
    let mut extra = [0_u8; 1];
    if input.read(&mut extra).await? != 0 {
        anyhow::bail!("planning IPC channel contained bytes after its frame");
    }
    Ok(bytes)
}

pub(super) fn read_frame_sync(mut input: impl Read, limit: u64) -> anyhow::Result<Vec<u8>> {
    let mut header = [0_u8; 8];
    input.read_exact(&mut header)?;
    let length = u64::from_be_bytes(header);
    check_frame_size("planning IPC frame bytes", limit, length)?;
    let mut bytes = vec![0_u8; usize::try_from(length)?];
    input.read_exact(&mut bytes)?;
    let mut extra = [0_u8; 1];
    if input.read(&mut extra)? != 0 {
        anyhow::bail!("planning IPC channel contained bytes after its frame");
    }
    Ok(bytes)
}

fn write_frame_sync(mut output: impl Write, bytes: &[u8]) -> io::Result<()> {
    output.write_all(&(bytes.len() as u64).to_be_bytes())?;
    output.write_all(bytes)?;
    output.flush()
}

async fn read_retained(mut input: impl AsyncRead + Unpin, limit: usize) -> io::Result<Vec<u8>> {
    let mut retained = Vec::with_capacity(limit);
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = input.read(&mut buffer).await?;
        if read == 0 {
            return Ok(retained);
        }
        let keep = read.min(limit.saturating_sub(retained.len()));
        retained.extend_from_slice(&buffer[..keep]);
    }
}

/// Applies the planner worker's OS resource backstops in-process.
///
/// The worker binary is ours and this runs before it reads any request, so a
/// pre-exec cap adds nothing over capping here (Windows already limits only in
/// the worker). `RLIMIT_AS` caps address space where the OS supports it; macOS
/// (Darwin) has no address-space rlimit and rejects the call with `EINVAL`, and
/// Windows uses a Job Object memory limit in the sibling below. `RLIMIT_CPU` caps
/// CPU time on all unix. Wall-clock time is enforced by the parent's tokio
/// timeout and `kill_on_drop`, not here.
#[cfg(unix)]
fn apply_planner_worker_limits() -> io::Result<()> {
    let cpu_seconds = planning_measurements().worker_wall_millis.div_ceil(1_000);
    #[cfg(not(target_os = "macos"))]
    set_resource_limit("RLIMIT_AS", libc::RLIMIT_AS, PLANNER_ADDRESS_SPACE_BYTES)?;
    set_resource_limit("RLIMIT_CPU", libc::RLIMIT_CPU, cpu_seconds)?;
    Ok(())
}

#[cfg(all(unix, target_os = "linux", target_env = "gnu"))]
type ResourceLimitKind = libc::__rlimit_resource_t;

#[cfg(all(unix, not(all(target_os = "linux", target_env = "gnu"))))]
type ResourceLimitKind = libc::c_int;

#[cfg(unix)]
fn set_resource_limit(name: &str, resource: ResourceLimitKind, value: u64) -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    // SAFETY: limit points to initialized storage for the requested resource.
    if unsafe { libc::setrlimit(resource, &limit) } == 0 {
        Ok(())
    } else {
        let error = io::Error::last_os_error();
        Err(io::Error::new(
            error.kind(),
            format!("failed to set planner {name} limit: {error}"),
        ))
    }
}

#[cfg(all(not(unix), not(windows)))]
fn apply_planner_worker_limits() -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn apply_planner_worker_limits() -> io::Result<()> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{JobObjects::*, Threading::GetCurrentProcess},
    };

    // SAFETY: all pointers refer to initialized storage for the duration of each call.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_PROCESS_TIME;
        // Receipt: Windows job CPU time uses 100 ns units, or 10,000,000 units per second.
        limits.BasicLimitInformation.PerProcessUserTimeLimit =
            planning_measurements().worker_wall_millis.div_ceil(1_000) as i64 * 10_000_000;
        limits.ProcessMemoryLimit = PLANNER_ADDRESS_SPACE_BYTES as usize;
        let configured = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        );
        if configured == 0 || AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            let error = io::Error::last_os_error();
            CloseHandle(job);
            return Err(error);
        }
        // Keep the job handle open for the worker process lifetime.
        Ok(())
    }
}
