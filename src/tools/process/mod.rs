mod tools;

use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{ChildStdin, Command},
    sync::{mpsc, Notify},
};
use uuid::Uuid;

pub use tools::{ListProcesses, PollProcess, StartProcess, StopProcess, WriteProcess};

#[derive(Clone, Debug)]
pub struct ProcessLimits {
    pub max_live: usize,
    pub max_records: usize,
    pub max_bytes: usize,
    pub max_chunks: usize,
    pub retention: Duration,
}
impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            max_live: 16,
            max_records: 64,
            max_bytes: 1024 * 1024,
            max_chunks: 8192,
            retention: Duration::from_secs(30 * 60),
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Starting,
    Running,
    Exited,
    Terminated,
    TimedOut,
    FailedToStart,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}
#[derive(Clone, Debug, Serialize)]
pub struct Chunk {
    pub cursor: u64,
    pub stream: Stream,
    pub text: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub process_id: String,
    pub command: String,
    pub state: State,
    pub runtime_seconds: f64,
    pub first_cursor: u64,
    pub next_cursor: u64,
    pub truncated: bool,
    pub chunks: Vec<Chunk>,
    pub exit_code: Option<i32>,
    pub terminal_detail: Option<String>,
}
struct Record {
    id: String,
    command: String,
    state: State,
    started: Instant,
    completed: Option<Instant>,
    chunks: VecDeque<Chunk>,
    bytes: usize,
    next: u64,
    exit_code: Option<i32>,
    detail: Option<String>,
    stdin: Option<ChildStdin>,
    stop: Option<mpsc::Sender<Duration>>,
    pid: Option<u32>,
    notify: Arc<Notify>,
}
struct Inner {
    records: HashMap<String, Arc<Mutex<Record>>>,
    limits: ProcessLimits,
}
#[derive(Clone)]
pub struct ProcessManager(Arc<Mutex<Inner>>);
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
        {
            let i = self.0.lock().unwrap();
            if i.records
                .values()
                .filter(|r| !terminal(r.lock().unwrap().state))
                .count()
                >= i.limits.max_live
            {
                return Err("live process limit reached".into());
            }
        }
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
            pid: None,
            notify,
        }));
        self.0
            .lock()
            .unwrap()
            .records
            .insert(id.clone(), rec.clone());
        let mut cmd = shell_command(&command);
        cmd.current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        match cmd.spawn() {
            Ok(mut child) => {
                let stdin = child.stdin.take();
                let stdout = child.stdout.take().unwrap();
                let stderr = child.stderr.take().unwrap();
                let (tx, rx) = mpsc::channel(64);
                let (stop_tx, stop_rx) = mpsc::channel(1);
                {
                    let mut r = rec.lock().unwrap();
                    r.state = State::Running;
                    r.stdin = stdin;
                    r.stop = Some(stop_tx);
                    r.pid = child.id();
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
    pub async fn poll(
        &self,
        id: &str,
        cursor: Option<u64>,
        wait: Duration,
    ) -> Result<Snapshot, String> {
        let rec = self.get(id)?;
        let cursor = cursor.unwrap_or(0);
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = {
                let r = rec.lock().unwrap();
                r.notify.clone().notified_owned()
            };
            let s = snapshot(&rec, cursor);
            if !s.chunks.is_empty() || terminal(s.state) || wait.is_zero() {
                return Ok(s);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(snapshot(&rec, cursor));
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
        tx.send(grace)
            .await
            .map_err(|_| "process already stopped".into())
    }
    pub fn list(&self) -> Vec<Snapshot> {
        self.prune();
        self.0
            .lock()
            .unwrap()
            .records
            .values()
            .map(|r| snapshot(r, 0))
            .collect()
    }
    fn get(&self, id: &str) -> Result<Arc<Mutex<Record>>, String> {
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
fn terminal(s: State) -> bool {
    matches!(
        s,
        State::Exited | State::Terminated | State::TimedOut | State::FailedToStart
    )
}
fn snapshot(rec: &Arc<Mutex<Record>>, cursor: u64) -> Snapshot {
    let r = rec.lock().unwrap();
    let first = r.chunks.front().map_or(r.next, |c| c.cursor);
    Snapshot {
        process_id: r.id.clone(),
        command: r.command.clone(),
        state: r.state,
        runtime_seconds: r.started.elapsed().as_secs_f64(),
        first_cursor: first,
        next_cursor: r.next,
        truncated: cursor < first,
        chunks: r
            .chunks
            .iter()
            .filter(|c| c.cursor >= cursor.max(first))
            .cloned()
            .collect(),
        exit_code: r.exit_code,
        terminal_detail: r.detail.clone(),
    }
}
#[cfg(unix)]
fn shell_command(command: &str) -> Command {
    let mut c = Command::new("bash");
    c.arg("-lc").arg(command);
    c
}
#[cfg(windows)]
fn shell_command(command: &str) -> Command {
    let mut c = Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
    c
}
async fn reader<R: AsyncRead + Unpin>(
    stream: Stream,
    mut r: R,
    tx: mpsc::Sender<(Stream, Vec<u8>)>,
) {
    let mut b = [0; 8192];
    while let Ok(n) = r.read(&mut b).await {
        if n == 0 {
            break;
        }
        if tx.send((stream, b[..n].to_vec())).await.is_err() {
            break;
        }
    }
}
#[expect(
    clippy::too_many_arguments,
    reason = "supervisor owns the child and its distinct I/O and control channels"
)]
async fn supervise(
    rec: Arc<Mutex<Record>>,
    mut child: tokio::process::Child,
    stdout: impl AsyncRead + Unpin + Send + 'static,
    stderr: impl AsyncRead + Unpin + Send + 'static,
    tx: mpsc::Sender<(Stream, Vec<u8>)>,
    mut rx: mpsc::Receiver<(Stream, Vec<u8>)>,
    mut stop: mpsc::Receiver<Duration>,
    timeout: Option<Duration>,
    limits: ProcessLimits,
) {
    tokio::spawn(reader(Stream::Stdout, stdout, tx.clone()));
    tokio::spawn(reader(Stream::Stderr, stderr, tx));
    let sleep = tokio::time::sleep(timeout.unwrap_or(Duration::MAX));
    tokio::pin!(sleep);
    let mut final_state = State::Exited;
    loop {
        tokio::select! {Some((stream,b))=rx.recv()=>push(&rec,stream,b,&limits),g=stop.recv()=>{final_state=State::Terminated;terminate(&mut child,g.unwrap_or_default()).await;break},_= &mut sleep=>{final_state=State::TimedOut;terminate(&mut child,Duration::from_secs(2)).await;break},s=child.wait()=>{{let mut r=rec.lock().unwrap();r.exit_code=s.ok().and_then(|x|x.code());}break}}
    }
    while let Some((s, b)) = rx.recv().await {
        push(&rec, s, b, &limits)
    }
    let mut r = rec.lock().unwrap();
    r.stdin = None;
    r.stop = None;
    r.state = final_state;
    r.completed = Some(Instant::now());
    r.notify.notify_waiters();
}
fn push(rec: &Arc<Mutex<Record>>, stream: Stream, b: Vec<u8>, limits: &ProcessLimits) {
    let mut r = rec.lock().unwrap();
    let len = b.len();
    let cursor = r.next;
    r.next += 1;
    r.bytes += len;
    r.chunks.push_back(Chunk {
        cursor,
        stream,
        text: String::from_utf8_lossy(&b).into_owned(),
    });
    while r.bytes > limits.max_bytes || r.chunks.len() > limits.max_chunks {
        if let Some(c) = r.chunks.pop_front() {
            r.bytes -= c.text.len()
        }
    }
    r.notify.notify_waiters();
}
#[cfg(unix)]
async fn terminate(child: &mut tokio::process::Child, grace: Duration) {
    if let Some(pid) = child.id().and_then(|p| i32::try_from(p).ok()) {
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
    }
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        if let Some(pid) = child.id().and_then(|p| i32::try_from(p).ok()) {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = child.wait().await;
    }
}
#[cfg(windows)]
async fn terminate(child: &mut tokio::process::Child, grace: Duration) {
    let _ = child.start_kill();
    let _ = tokio::time::timeout(grace, child.wait()).await;
}
impl Drop for Inner {
    fn drop(&mut self) {
        #[cfg(unix)]
        for r in self.records.values() {
            if !terminal(r.lock().unwrap().state) {
                if let Some(pid) = process_pid(r) {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
            }
        }
    }
}
#[cfg(unix)]
fn process_pid(r: &Arc<Mutex<Record>>) -> Option<i32> {
    r.lock()
        .unwrap()
        .pid
        .and_then(|pid| i32::try_from(pid).ok())
}
