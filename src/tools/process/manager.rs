use super::{
    platform::{shell_command, ProcessTree},
    supervisor::supervise,
    types::{terminal, ProcessLimits},
    Chunk, ProcessSummary, Snapshot, State,
};
use crate::tool::ToolShutdown;
use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    path::Path,
    pin::Pin,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncWriteExt,
    process::ChildStdin,
    sync::{mpsc, Notify},
};
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
    pub(super) stdin: Option<ChildStdin>,
    pub(super) stop: Option<mpsc::UnboundedSender<Duration>>,
    pub(super) tree: Option<Arc<ProcessTree>>,
    pub(super) notify: Arc<Notify>,
}
struct Inner {
    records: HashMap<String, SharedRecord>,
    limits: ProcessLimits,
}
#[derive(Clone)]
pub struct ProcessManager(Arc<Mutex<Inner>>);

impl ToolShutdown for ProcessManager {
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(ProcessManager::shutdown(self))
    }
}
impl ProcessManager {
    pub fn new(limits: ProcessLimits) -> Self {
        Self(Arc::new(Mutex::new(Inner {
            records: HashMap::new(),
            limits,
        })))
    }
    pub async fn start(
        &self,
        command: String,
        cwd: &Path,
        timeout: Option<Duration>,
    ) -> Result<Snapshot, String> {
        self.prune();
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
            stdin: None,
            stop: None,
            tree: None,
            notify,
        }));
        {
            let mut inner = self.0.lock().unwrap();
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
        let mut cmd = shell_command(&command);
        cmd.current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
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
                let stdin = child.stdin.take();
                let stdout = child.stdout.take().unwrap();
                let stderr = child.stderr.take().unwrap();
                let (tx, rx) = mpsc::channel(64);
                let (stop_tx, stop_rx) = mpsc::unbounded_channel();
                {
                    let mut r = rec.lock().unwrap();
                    r.state = State::Running;
                    r.stdin = stdin;
                    r.stop = Some(stop_tx);
                    r.tree = Some(tree.clone());
                    r.notify.notify_waiters();
                }
                let limits = self.0.lock().unwrap().limits.clone();
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
    pub async fn write(&self, id: &str, text: &str, close: bool) -> Result<(), String> {
        let rec = self.get(id)?;
        let mut stdin = {
            let mut r = rec.lock().unwrap();
            if terminal(r.state) {
                return Err("process has exited".into());
            }
            r.stdin.take().ok_or("stdin is closed")?
        };
        if !text.is_empty() {
            stdin
                .write_all(text.as_bytes())
                .await
                .map_err(|e| e.to_string())?
        }
        if close {
            stdin.shutdown().await.map_err(|e| e.to_string())?
        } else {
            rec.lock().unwrap().stdin = Some(stdin)
        }
        Ok(())
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
    pub fn list(&self) -> Vec<ProcessSummary> {
        self.prune();
        self.0
            .lock()
            .unwrap()
            .records
            .values()
            .map(summary)
            .collect()
    }
    pub async fn shutdown(&self) {
        let records = self
            .0
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
        self.0
            .lock()
            .unwrap()
            .records
            .get(id)
            .cloned()
            .ok_or_else(|| "unknown process_id".into())
    }
    fn prune(&self) {
        let mut i = self.0.lock().unwrap();
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
fn snapshot(rec: &SharedRecord, cursor: u64) -> Snapshot {
    snapshot_bounded(rec, cursor, usize::MAX)
}
fn snapshot_bounded(rec: &SharedRecord, cursor: u64, max_output_bytes: usize) -> Snapshot {
    let r = rec.lock().unwrap();
    let first = r.chunks.front().map_or(r.next, |chunk| chunk.chunk.cursor);
    let requested = cursor.max(first);
    let mut chunks = Vec::new();
    for retained in r
        .chunks
        .iter()
        .filter(|item| item.chunk.cursor >= requested)
    {
        chunks.push(retained.chunk.clone());
        if serde_json::to_vec(&chunks).map_or(true, |json| json.len() > max_output_bytes) {
            chunks.pop();
            break;
        }
    }
    let next_cursor = chunks.last().map_or(requested, |chunk| chunk.cursor + 1);
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

fn summary(rec: &SharedRecord) -> ProcessSummary {
    let r = rec.lock().unwrap();
    let first = r.chunks.front().map_or(r.next, |c| c.chunk.cursor);
    ProcessSummary {
        process_id: r.id.clone(),
        command: r.command.clone(),
        state: r.state,
        runtime_seconds: r.started.elapsed().as_secs_f64(),
        first_cursor: first,
        next_cursor: r.next,
        exit_code: r.exit_code,
        terminal_detail: r.detail.clone(),
    }
}
