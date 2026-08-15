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
    pub(super) observed: bool,
}
struct Inner {
    records: HashMap<String, SharedRecord>,
    limits: ProcessLimits,
}
#[derive(Clone)]
pub struct ProcessManager {
    inner: Arc<Mutex<Inner>>,
    environment: ProcessEnvironment,
    exited: Arc<Notify>,
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
        let exited = Arc::new(Notify::new());
        Self {
            inner: Arc::new(Mutex::new(Inner {
                records: HashMap::new(),
                limits,
            })),
            environment,
            exited,
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
            rho_tools::shell_invocation(command),
            self.environment.clone(),
            ProcessOutputLimits::new(1, timeout),
        );
        self.start_execution(execution).await
    }

    /// Starts a process from an already authorized execution plan.
    pub async fn start_execution(&self, execution: ProcessExecution) -> Result<Snapshot, String> {
        self.prune();
        // Build the spawn plan before registering a live record so setup failures
        // cannot leave a Starting entry with no completion.
        let command = record_command(execution.invocation())?;
        let mut cmd = command_from_execution(&execution)?;
        cmd.current_dir(execution.working_directory())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        super::prepare_child_command(&mut cmd);
        rho_tools::apply_process_environment(&mut cmd, execution.environment())?;
        let timeout = execution.output_limits().timeout();

        let id = Uuid::new_v4().to_string();
        let notify = Arc::new(Notify::new());
        let rec = Arc::new(Mutex::new(Record {
            id: id.clone(),
            command,
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
            observed: false,
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
        match cmd.spawn() {
            Ok(mut child) => {
                let tree = match ProcessTree::attach(&child) {
                    Ok(tree) => Arc::new(tree),
                    Err(error) => {
                        let _ = child.start_kill();
                        mark_terminal(&rec, State::FailedToStart, Some(error), &self.exited);
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
                    self.exited.clone(),
                ));
                Ok(snapshot(&rec, 0))
            }
            Err(e) => {
                mark_terminal(
                    &rec,
                    State::FailedToStart,
                    Some(e.to_string()),
                    &self.exited,
                );
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
                if terminal(s.state) {
                    rec.lock().unwrap().observed = true;
                }
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
    /// Fires when any process becomes terminal. Subscribe before checking
    /// pending notifications so an exit during the wait is not lost.
    pub fn notified_owned(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        self.exited.clone().notified_owned()
    }

    pub fn has_active_or_pending_notification(&self) -> bool {
        self.inner.lock().unwrap().records.values().any(|record| {
            let record = record.lock().unwrap();
            !terminal(record.state) || !record.observed
        })
    }

    /// Drains unobserved terminal processes, oldest first.
    pub fn take_notifications(&self) -> Vec<super::ProcessNotification> {
        let inner = self.inner.lock().unwrap();
        let mut notifications = inner
            .records
            .values()
            .filter_map(|record| {
                let mut record = record.lock().unwrap();
                if !terminal(record.state) || record.observed {
                    return None;
                }
                record.observed = true;
                Some((
                    record.started,
                    super::ProcessNotification {
                        process_id: record.id.clone(),
                        command: record.command.clone(),
                        state: record.state,
                        exit_code: record.exit_code,
                        output: super::notify::excerpt_output(
                            &record
                                .chunks
                                .iter()
                                .map(|item| item.chunk.clone())
                                .collect::<Vec<_>>(),
                            super::notify::output_excerpt_budget(),
                        ),
                        terminal_detail: record.detail.clone(),
                    },
                ))
            })
            .collect::<Vec<_>>();
        notifications.sort_by(|(a_started, a), (b_started, b)| {
            a_started
                .cmp(b_started)
                .then_with(|| a.process_id.cmp(&b.process_id))
        });
        notifications
            .into_iter()
            .map(|(_, notification)| notification)
            .collect()
    }

    pub fn restore_notifications(&self, notifications: &[super::ProcessNotification]) {
        if notifications.is_empty() {
            return;
        }
        let inner = self.inner.lock().unwrap();
        for notification in notifications {
            let Some(record) = inner.records.get(&notification.process_id) else {
                continue;
            };
            let mut record = record.lock().unwrap();
            if terminal(record.state) && record.observed {
                record.observed = false;
            }
        }
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

fn record_command(invocation: &ProcessInvocation) -> Result<String, String> {
    if let Some(command) = invocation.shell_command() {
        return Ok(command.to_string());
    }
    match invocation {
        ProcessInvocation::Executable {
            executable,
            arguments,
            ..
        } => {
            let mut label = executable.display().to_string();
            for argument in arguments {
                label.push(' ');
                label.push_str(argument);
            }
            Ok(label)
        }
        _ => Err("unsupported process invocation".into()),
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
            Ok(cmd)
        }
        ProcessInvocation::Executable {
            executable,
            arguments,
            ..
        } => {
            let mut cmd = tokio::process::Command::new(executable);
            cmd.args(arguments);
            Ok(cmd)
        }
        _ => Err("unsupported process invocation".into()),
    }
}

fn mark_terminal(rec: &SharedRecord, state: State, detail: Option<String>, exited: &Notify) {
    let mut record = rec.lock().unwrap();
    record.state = state;
    record.detail = detail;
    record.completed = Some(Instant::now());
    record.notify.notify_waiters();
    drop(record);
    exited.notify_waiters();
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
