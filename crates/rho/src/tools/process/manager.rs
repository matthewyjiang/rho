use super::{
    platform::ProcessTree,
    supervisor::supervise,
    types::{terminal, ProcessLimits},
    Chunk, Snapshot, State,
};
use rho_sdk::{ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits};
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, Notify};
use uuid::Uuid;
pub(super) type SharedRecord = Arc<Mutex<Record>>;
pub(super) struct RetainedChunk {
    pub(super) chunk: Chunk,
    pub(super) byte_cost: usize,
}
pub(super) struct Record {
    pub(super) id: String,
    pub(super) command: String,
    pub(super) state: State,
    pub(super) started: Instant,
    pub(super) completed: Option<Instant>,
    pub(super) chunks: VecDeque<RetainedChunk>,
    pub(super) bytes: usize,
    pub(super) next: u64,
    pub(super) exit_code: Option<i32>,
    pub(super) detail: Option<String>,
    pub(super) stop: Option<mpsc::UnboundedSender<Duration>>,
    pub(super) tree: Option<Arc<ProcessTree>>,
    pub(super) notify: Arc<Notify>,
}
struct Inner {
    records: HashMap<String, SharedRecord>,
    limits: ProcessLimits,
}
#[derive(Clone)]
pub struct ProcessManager {
    inner: Arc<Mutex<Inner>>,
    environment: ProcessEnvironment,
}

impl ProcessManager {
    /// Creates a manager that inherits the full ambient environment.
    ///
    /// Prefer [`Self::with_environment`] at composition roots that need a
    /// stricter child-process policy.
    #[cfg(test)]
    pub fn new(limits: ProcessLimits) -> Self {
        Self::with_environment(limits, ProcessEnvironment::InheritAll)
    }

    pub fn with_environment(limits: ProcessLimits, environment: ProcessEnvironment) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                records: HashMap::new(),
                limits,
            })),
            environment,
        }
    }

    pub async fn start(
        &self,
        command: String,
        cwd: &Path,
        timeout: Option<Duration>,
    ) -> Result<Snapshot, String> {
        let execution = ProcessExecution::new(
            cwd,
            process_invocation(&command),
            self.environment.clone(),
            ProcessOutputLimits::new(1, timeout),
        );
        self.start_execution(execution).await
    }

    /// Starts a process from an already authorized execution plan.
    pub async fn start_execution(&self, execution: ProcessExecution) -> Result<Snapshot, String> {
        self.prune();
        let command = execution
            .invocation()
            .shell_command()
            .ok_or_else(|| "process manager requires a shell invocation".to_string())?
            .to_string();
        let id = Uuid::new_v4().to_string();
        let notify = Arc::new(Notify::new());
        let rec = Arc::new(Mutex::new(Record {
            id: id.clone(),
            command: command.clone(),
            state: State::Starting,
            started: Instant::now(),
            completed: None,
            chunks: VecDeque::new(),
            bytes: 0,
            next: 0,
            exit_code: None,
            detail: None,
            stop: None,
            tree: None,
            notify,
        }));
        {
            let mut inner = self.inner.lock().unwrap();
            let live = inner
                .records
                .values()
                .filter(|record| !terminal(record.lock().unwrap().state))
                .count();
            if live >= inner.limits.max_live {
                return Err("live process limit reached".into());
            }
            inner.records.insert(id.clone(), rec.clone());
        }
        let mut cmd = command_from_execution(&execution)?;
        cmd.current_dir(execution.working_directory())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        rho_tools::apply_process_environment(&mut cmd, execution.environment())?;
        let timeout = execution.output_limits().timeout();
        match cmd.spawn() {
            Ok(mut child) => {
                let tree = match ProcessTree::attach(&child) {
                    Ok(tree) => Arc::new(tree),
                    Err(error) => {
                        let _ = child.start_kill();
                        let mut r = rec.lock().unwrap();
                        r.state = State::FailedToStart;
                        r.detail = Some(error);
                        r.completed = Some(Instant::now());
                        return Ok(snapshot(&rec, 0));
                    }
                };
                let stdout = child.stdout.take().unwrap();
                let stderr = child.stderr.take().unwrap();
                let (tx, rx) = mpsc::channel(64);
                let (stop_tx, stop_rx) = mpsc::unbounded_channel();
                {
                    let mut r = rec.lock().unwrap();
                    r.state = State::Running;
                    r.stop = Some(stop_tx);
                    r.tree = Some(tree.clone());
                    r.notify.notify_waiters();
                }
                let limits = self.inner.lock().unwrap().limits.clone();
                tokio::spawn(supervise(
                    rec.clone(),
                    child,
                    stdout,
                    stderr,
                    tx,
                    rx,
                    stop_rx,
                    timeout,
                    limits,
                    tree,
                ));
                Ok(snapshot(&rec, 0))
            }
            Err(e) => {
                {
                    let mut r = rec.lock().unwrap();
                    r.state = State::FailedToStart;
                    r.detail = Some(e.to_string());
                    r.completed = Some(Instant::now());
                    r.notify.notify_waiters();
                }
                Ok(snapshot(&rec, 0))
            }
        }
    }
    #[cfg(test)]
    pub async fn poll(
        &self,
        id: &str,
        cursor: Option<u64>,
        wait: Duration,
    ) -> Result<Snapshot, String> {
        self.poll_bounded(id, cursor, wait, usize::MAX).await
    }
    pub async fn poll_bounded(
        &self,
        id: &str,
        cursor: Option<u64>,
        wait: Duration,
        max_output_bytes: usize,
    ) -> Result<Snapshot, String> {
        let rec = self.get(id)?;
        let cursor = cursor.unwrap_or(0);
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = {
                let r = rec.lock().unwrap();
                r.notify.clone().notified_owned()
            };
            let s = snapshot_bounded(&rec, cursor, max_output_bytes);
            if !s.chunks.is_empty() || terminal(s.state) || wait.is_zero() {
                return Ok(s);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(snapshot_bounded(&rec, cursor, max_output_bytes));
            }
        }
    }
    pub async fn stop(&self, id: &str, grace: Duration) -> Result<(), String> {
        let tx = {
            let r = self.get(id)?;
            let r = r.lock().unwrap();
            if terminal(r.state) {
                return Err("process has exited".into());
            }
            r.stop.clone().ok_or("process is starting")?
        };
        tx.send(grace).map_err(|_| "process already stopped".into())
    }
    pub async fn shutdown(&self) {
        let records = self
            .inner
            .lock()
            .unwrap()
            .records
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let requests = records
            .iter()
            .filter_map(|record| record.lock().unwrap().stop.clone())
            .collect::<Vec<_>>();
        for request in requests {
            let _ = request.send(Duration::ZERO);
        }
        for record in records {
            loop {
                let notified = {
                    let record = record.lock().unwrap();
                    if terminal(record.state) {
                        break;
                    }
                    record.notify.clone().notified_owned()
                };
                notified.await;
            }
        }
    }

    fn get(&self, id: &str) -> Result<SharedRecord, String> {
        self.inner
            .lock()
            .unwrap()
            .records
            .get(id)
            .cloned()
            .ok_or_else(|| "unknown process_id".into())
    }
    fn prune(&self) {
        let mut i = self.inner.lock().unwrap();
        let retention = i.limits.retention;
        i.records.retain(|_, r| {
            r.lock()
                .unwrap()
                .completed
                .is_none_or(|t| t.elapsed() < retention)
        });
        if i.records.len() > i.limits.max_records {
            let mut done = i
                .records
                .iter()
                .filter_map(|(k, r)| r.lock().unwrap().completed.map(|t| (k.clone(), t)))
                .collect::<Vec<_>>();
            done.sort_by_key(|x| x.1);
            for (k, _) in done
                .into_iter()
                .take(i.records.len() - i.limits.max_records)
            {
                i.records.remove(&k);
            }
        }
    }
}

fn command_from_execution(execution: &ProcessExecution) -> Result<tokio::process::Command, String> {
    match execution.invocation() {
        ProcessInvocation::Shell {
            executable,
            arguments,
            command,
            ..
        } => {
            let mut cmd = tokio::process::Command::new(executable);
            cmd.args(arguments).arg(command);
            #[cfg(unix)]
            cmd.process_group(0);
            Ok(cmd)
        }
        ProcessInvocation::Executable {
            executable,
            arguments,
            ..
        } => {
            let mut cmd = tokio::process::Command::new(executable);
            cmd.args(arguments);
            #[cfg(unix)]
            cmd.process_group(0);
            Ok(cmd)
        }
        _ => Err("unsupported process invocation".into()),
    }
}

#[cfg(unix)]
fn process_invocation(command: &str) -> ProcessInvocation {
    ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], command.to_string())
}

#[cfg(windows)]
fn process_invocation(command: &str) -> ProcessInvocation {
    ProcessInvocation::shell_from_path(
        "powershell.exe",
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
        ],
        rho_tools::powershell::wrapped_command(command),
    )
}

fn snapshot(rec: &SharedRecord, cursor: u64) -> Snapshot {
    snapshot_bounded(rec, cursor, usize::MAX)
}
fn snapshot_bounded(rec: &SharedRecord, cursor: u64, max_output_bytes: usize) -> Snapshot {
    let r = rec.lock().unwrap();
    let first = r.chunks.front().map_or(r.next, |chunk| chunk.chunk.cursor);
    let requested = cursor.max(first);
    let mut next_cursor = requested;
    let mut chunks = Vec::new();
    for retained in r
        .chunks
        .iter()
        .filter(|item| item.chunk.cursor >= requested)
    {
        chunks.push(retained.chunk.clone());
        if serde_json::to_vec(&chunks).map_or(true, |json| json.len() > max_output_bytes) {
            chunks.pop();
            if chunks.is_empty() {
                // A chunk that cannot fit by itself must still be consumed, or
                // every poll will remain stuck on it.
                next_cursor = retained.chunk.cursor + 1;
            }
            break;
        }
        next_cursor = retained.chunk.cursor + 1;
    }
    Snapshot {
        process_id: r.id.clone(),
        command: r.command.clone(),
        state: r.state,
        runtime_seconds: r.started.elapsed().as_secs_f64(),
        first_cursor: first,
        next_cursor,
        available_cursor: r.next,
        truncated: cursor < first,
        output_pending: next_cursor < r.next,
        chunks,
        exit_code: r.exit_code,
        terminal_detail: r.detail.clone(),
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        for record in self.records.values() {
            if let Some(tree) = record.lock().unwrap().tree.clone() {
                tree.kill();
            }
        }
    }
}
