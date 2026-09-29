use serde_json::json;
use std::{
    env,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const REQUEST_TIMEOUT: Duration = Duration::from_millis(500);
#[cfg(unix)]
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const AGENT: &str = "rho";
/// Herdr's `resume_argv` limits (`validate_resume_argv` in Herdr 0.9.2). Herdr
/// rejects the whole report when they are exceeded, so Rho drops the command
/// instead of losing the state update.
const MAX_RESUME_ARGS: usize = 64;
const MAX_RESUME_ARGV_BYTES: usize = 8 * 1024;

/// Last `seq` sent from this process. Shared by every reporter clone so reports
/// stay ordered however they are spawned.
static LAST_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HerdrReporter {
    config: Option<HerdrConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HerdrConfig {
    socket_path: PathBuf,
    pane_id: String,
    source: HerdrSource,
}

/// Which Rho process kind is reporting. Herdr only lets a source release its
/// own claim, so a `rho run` started from a tool inside an interactive pane
/// cannot release the interactive session or drop its resume command.
///
/// Herdr reserves the `herdr:` prefix for its own integrations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HerdrSource {
    /// The interactive TUI and `rho attach`.
    #[default]
    Interactive,
    /// `rho run` and `rho acp`.
    Headless,
}

impl HerdrSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Interactive => "rho",
            Self::Headless => "rho-headless",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HerdrState {
    Idle,
    Working,
    Blocked,
}

/// The Rho session a report belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HerdrSession {
    pub id: String,
    /// Command Herdr runs in the restored pane after a server restart to reopen
    /// this session. `None` when the session cannot be resumed from a shell.
    pub resume_argv: Option<Vec<String>>,
}

impl HerdrSession {
    /// A session reference without a resume command.
    pub fn without_resume(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            resume_argv: None,
        }
    }
}

/// Whether Herdr answered a report without an error. Callers that must keep
/// Herdr in sync retry on [`Self::Failed`]; the rest ignore it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum HerdrDelivery {
    Accepted,
    Failed,
}

impl HerdrDelivery {
    fn from_exchange(response: std::io::Result<Vec<u8>>) -> Self {
        let accepted = response.is_ok_and(|response| {
            serde_json::from_slice::<serde_json::Value>(&response)
                .is_ok_and(|value| value.get("error").is_none())
        });
        if accepted {
            Self::Accepted
        } else {
            Self::Failed
        }
    }
}

/// A report whose `seq` is already allocated.
#[derive(Debug)]
pub struct HerdrReport(serde_json::Value);

impl HerdrState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
        }
    }
}

impl HerdrReporter {
    pub fn from_env() -> Self {
        Self::from_env_vars(|key| env::var(key).ok())
    }

    pub(crate) fn from_env_vars(mut get_var: impl FnMut(&str) -> Option<String>) -> Self {
        let enabled = platform_supported() && get_var("HERDR_ENV").as_deref() == Some("1");
        let socket_path = get_var("HERDR_SOCKET_PATH").filter(|value| !value.is_empty());
        let pane_id = get_var("HERDR_PANE_ID").filter(|value| !value.is_empty());
        let config = enabled
            .then_some((socket_path, pane_id))
            .and_then(|(socket_path, pane_id)| Some((socket_path?, pane_id?)))
            .map(|(socket_path, pane_id)| HerdrConfig {
                socket_path: PathBuf::from(socket_path),
                pane_id,
                source: HerdrSource::default(),
            });
        Self { config }
    }

    /// The same reporter under another source.
    pub fn with_source(mut self, source: HerdrSource) -> Self {
        if let Some(config) = &mut self.config {
            config.source = source;
        }
        self
    }

    pub fn is_enabled(&self) -> bool {
        self.config.is_some()
    }

    pub fn socket_is_reachable(&self) -> Option<bool> {
        let config = self.config.as_ref()?;
        Some(socket_is_reachable(&config.socket_path))
    }

    /// Reports agent state. Carrying the session also holds the pane for Rho,
    /// which Herdr requires before it accepts a resume command.
    pub async fn report_state(
        &self,
        state: HerdrState,
        message: Option<&str>,
        session: Option<&HerdrSession>,
    ) -> HerdrDelivery {
        match self.state_report(state, message, session) {
            Some(report) => self.send(report).await,
            None => HerdrDelivery::Failed,
        }
    }

    /// Builds a state report now so its `seq` orders it against reports built
    /// later, even when it is sent from a spawned task.
    pub fn state_report(
        &self,
        state: HerdrState,
        message: Option<&str>,
        session: Option<&HerdrSession>,
    ) -> Option<HerdrReport> {
        let config = self.config.as_ref()?;
        let mut params = json!({
            "pane_id": config.pane_id,
            "source": config.source.as_str(),
            "agent": AGENT,
            "state": state.as_str(),
            "seq": next_seq(),
        });
        if let Some(message) = message {
            params["message"] = json!(message);
        }
        if let Some(session) = session {
            add_session_params(&mut params, session);
        }
        Some(HerdrReport(json_rpc_request("pane.report_agent", params)))
    }

    /// Sends a report built by [`Self::state_report`].
    pub async fn send(&self, report: HerdrReport) -> HerdrDelivery {
        HerdrDelivery::from_exchange(self.exchange(report.0).await)
    }

    /// Reports a session change without changing agent state.
    pub async fn report_session(&self, session: &HerdrSession) -> HerdrDelivery {
        let Some(config) = &self.config else {
            return HerdrDelivery::Failed;
        };

        let mut params = json!({
            "pane_id": config.pane_id,
            "source": config.source.as_str(),
            "agent": AGENT,
            "seq": next_seq(),
        });
        add_session_params(&mut params, session);
        HerdrDelivery::from_exchange(
            self.exchange(json_rpc_request("pane.report_agent_session", params))
                .await,
        )
    }

    pub async fn release(&self) -> HerdrDelivery {
        let Some(config) = &self.config else {
            return HerdrDelivery::Failed;
        };

        let response = self
            .exchange(json_rpc_request(
                "pane.release_agent",
                json!({
                    "pane_id": config.pane_id,
                    "source": config.source.as_str(),
                    "agent": AGENT,
                    "seq": next_seq(),
                }),
            ))
            .await;
        HerdrDelivery::from_exchange(response)
    }

    async fn exchange(&self, request: serde_json::Value) -> std::io::Result<Vec<u8>> {
        let Some(config) = &self.config else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "herdr is not configured",
            ));
        };
        let payload = match serde_json::to_vec(&request) {
            Ok(mut payload) => {
                payload.push(b'\n');
                payload
            }
            Err(error) => {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error));
            }
        };
        exchange_payload(config.socket_path.clone(), payload).await
    }
}

fn add_session_params(params: &mut serde_json::Value, session: &HerdrSession) {
    params["agent_session_id"] = json!(session.id);
    match session.resume_argv.as_deref() {
        Some(argv) if resume_argv_is_valid(argv) => params["resume_argv"] = json!(argv),
        Some(argv) => {
            tracing::debug!(?argv, "herdr resume command dropped: outside herdr limits");
        }
        None => {}
    }
}

/// Mirrors Herdr's `resume_argv` rules: a bare command name first, no
/// apostrophes or control characters, and bounded size.
pub(crate) fn resume_argv_is_valid(argv: &[String]) -> bool {
    let Some(command) = argv.first() else {
        return false;
    };
    let plain_command = !command.is_empty()
        && !command.starts_with('-')
        && command
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    plain_command
        && argv.len() <= MAX_RESUME_ARGS
        && argv.iter().map(String::len).sum::<usize>() <= MAX_RESUME_ARGV_BYTES
        && !argv
            .iter()
            .any(|arg| arg.contains('\'') || arg.chars().any(char::is_control))
}

/// Herdr ignores a report whose `seq` is not above the last one it accepted
/// from this source, including across Rho restarts in the same pane. Wall-clock
/// microseconds carry the order across processes; the counter keeps it strictly
/// increasing within one. A backward clock step makes a new Rho process in the
/// same pane go unheard until real time passes the old value.
fn next_seq() -> u64 {
    let now = u64::try_from(request_id_suffix()).unwrap_or(u64::MAX);
    let previous = LAST_SEQ
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
            Some(now.max(last.saturating_add(1)))
        })
        .unwrap_or_else(|last| last);
    now.max(previous.saturating_add(1))
}

#[cfg(unix)]
fn socket_is_reachable(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
fn socket_is_reachable(_path: &Path) -> bool {
    false
}

fn json_rpc_request(method: &str, params: serde_json::Value) -> serde_json::Value {
    json!({
        "id": format!("{AGENT}:{}", request_id_suffix()),
        "method": method,
        "params": params,
    })
}

fn request_id_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros())
        .unwrap_or_default()
}

fn platform_supported() -> bool {
    cfg!(unix)
}

#[cfg(unix)]
async fn exchange_payload(socket_path: PathBuf, payload: Vec<u8>) -> std::io::Result<Vec<u8>> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    tokio::time::timeout(REQUEST_TIMEOUT, async move {
        let stream = tokio::net::UnixStream::connect(socket_path).await?;
        let (reader, mut writer) = stream.into_split();
        writer.write_all(&payload).await?;
        writer.shutdown().await?;
        let mut reader = BufReader::new(reader).take(MAX_RESPONSE_BYTES);
        let mut response = Vec::new();
        reader.read_until(b'\n', &mut response).await?;
        if response.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "herdr closed without a response",
            ));
        }
        Ok(response)
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "herdr request timed out"))?
}

#[cfg(not(unix))]
async fn exchange_payload(_socket_path: PathBuf, _payload: Vec<u8>) -> std::io::Result<Vec<u8>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "herdr socket transport is unix-only",
    ))
}

#[cfg(all(test, unix))]
pub(crate) mod test_support {
    use std::path::Path;

    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::HerdrReporter;

    pub(crate) fn reporter_for_socket(socket_path: &Path) -> HerdrReporter {
        let socket_path = socket_path.to_string_lossy().to_string();
        HerdrReporter::from_env_vars(|key| match key {
            "HERDR_ENV" => Some("1".into()),
            "HERDR_SOCKET_PATH" => Some(socket_path.clone()),
            "HERDR_PANE_ID" => Some("w1:p1".into()),
            _ => None,
        })
    }

    pub(crate) struct TestHerdrServer {
        requests: tokio::sync::mpsc::UnboundedReceiver<Value>,
    }

    impl TestHerdrServer {
        pub(crate) async fn bind(socket_path: &Path) -> Self {
            let listener = tokio::net::UnixListener::bind(socket_path).unwrap();
            let (tx, requests) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let mut stream = BufReader::new(stream);
                        let mut line = String::new();
                        stream.read_line(&mut line).await.unwrap();
                        let request = serde_json::from_str(&line).unwrap();
                        tx.send(request).unwrap();
                        // Keep the connection open after the framed response so
                        // clients must read a newline rather than waiting for EOF.
                        stream.get_mut().write_all(b"{}\n").await.unwrap();
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    });
                }
            });
            Self { requests }
        }

        pub(crate) async fn next_request(&mut self) -> Value {
            self.requests.recv().await.unwrap()
        }
    }
}

#[cfg(test)]
#[path = "herdr_tests.rs"]
mod tests;
