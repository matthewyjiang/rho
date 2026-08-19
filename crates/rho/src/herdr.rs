use serde_json::json;
use std::{
    env,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const REQUEST_TIMEOUT: Duration = Duration::from_millis(500);
const GRAPHICS_PROBE_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const SOURCE: &str = "herdr:rho";
const AGENT: &str = "rho";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HerdrReporter {
    config: Option<HerdrConfig>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HerdrConfig {
    socket_path: PathBuf,
    pane_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HerdrState {
    Idle,
    Working,
    Blocked,
}

/// Whether the active Herdr client can paint Kitty graphics placements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HerdrGraphicsCapability {
    /// Not running under a configured Herdr pane.
    NotHerdr,
    /// Herdr can paint Kitty placements for this pane.
    Paintable,
    /// Under Herdr, but host cell metrics or the probe path is unavailable.
    Unpaintable,
}

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
            });
        Self { config }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.is_some()
    }

    pub fn socket_is_reachable(&self) -> Option<bool> {
        let config = self.config.as_ref()?;
        Some(socket_is_reachable(&config.socket_path))
    }

    /// Probes whether the active Herdr client can paint Kitty placements.
    ///
    /// Herdr intercepts Kitty graphics from pane PTYs. Painting needs host cell
    /// metrics; when those are missing, Rho should keep image previews in the
    /// character grid (halfblocks) instead of reserving blank Kitty rows.
    pub async fn graphics_capability(&self) -> HerdrGraphicsCapability {
        let Some(config) = &self.config else {
            return HerdrGraphicsCapability::NotHerdr;
        };
        match probe_kitty_graphics(config).await {
            true => HerdrGraphicsCapability::Paintable,
            false => HerdrGraphicsCapability::Unpaintable,
        }
    }

    pub async fn report_state(
        &self,
        state: HerdrState,
        message: Option<&str>,
        session_id: Option<&str>,
    ) {
        let Some(config) = &self.config else {
            return;
        };

        let mut params = json!({
            "pane_id": config.pane_id,
            "source": SOURCE,
            "agent": AGENT,
            "state": state.as_str(),
        });
        if let Some(message) = message {
            params["message"] = json!(message);
        }
        if let Some(session_id) = session_id {
            params["agent_session_id"] = json!(session_id);
        }

        let _ = self
            .exchange(
                json_rpc_request("pane.report_agent", params),
                REQUEST_TIMEOUT,
            )
            .await;
    }

    pub async fn report_session(&self, session_id: Option<&str>) {
        let (Some(config), Some(session_id)) = (&self.config, session_id) else {
            return;
        };

        let _ = self
            .exchange(
                json_rpc_request(
                    "pane.report_agent_session",
                    json!({
                        "pane_id": config.pane_id,
                        "source": SOURCE,
                        "agent": AGENT,
                        "agent_session_id": session_id,
                    }),
                ),
                REQUEST_TIMEOUT,
            )
            .await;
    }

    pub async fn release(&self) {
        let Some(config) = &self.config else {
            return;
        };

        let _ = self
            .exchange(
                json_rpc_request(
                    "pane.release_agent",
                    json!({
                        "pane_id": config.pane_id,
                        "source": SOURCE,
                        "agent": AGENT,
                    }),
                ),
                REQUEST_TIMEOUT,
            )
            .await;
    }

    /// Splits a sibling pane and submits one shell command to it.
    ///
    /// The split and input requests use the bounded Herdr socket transport. If
    /// input submission fails after the split, Rho closes the new pane before
    /// returning the error.
    ///
    /// Interactive attach no longer uses this by default. The method stays so a
    /// host or a later "open in pane" action can still split a `rho attach`
    /// process.
    #[allow(dead_code)]
    pub(crate) async fn open_sibling_pane(&self, command: &str) -> std::io::Result<()> {
        let Some(config) = &self.config else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "herdr is not configured",
            ));
        };
        let split = self
            .request(
                "pane.split",
                json!({
                    "direction": "right",
                    "focus": false,
                    "target_pane_id": config.pane_id,
                }),
            )
            .await?;
        let pane_id = split
            .pointer("/pane/pane_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "herdr pane split returned no pane id",
                )
            })?
            .to_owned();

        if let Err(run_error) = self
            .request(
                "pane.send_input",
                json!({
                    "pane_id": pane_id,
                    "text": command,
                    "keys": ["enter"],
                }),
            )
            .await
        {
            let cleanup = self
                .request("pane.close", json!({ "pane_id": pane_id }))
                .await;
            return match cleanup {
                Ok(_) => Err(run_error),
                Err(cleanup_error) => Err(std::io::Error::other(format!(
                    "{run_error}; pane cleanup failed: {cleanup_error}"
                ))),
            };
        }
        Ok(())
    }

    async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> std::io::Result<serde_json::Value> {
        let response = self
            .exchange(json_rpc_request(method, params), REQUEST_TIMEOUT)
            .await?;
        let response: serde_json::Value = serde_json::from_slice(&response)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        if let Some(error) = response.get("error") {
            let message = error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| error.to_string());
            return Err(std::io::Error::other(message));
        }
        response.get("result").cloned().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "herdr response returned no result",
            )
        })
    }

    async fn exchange(
        &self,
        request: serde_json::Value,
        timeout: Duration,
    ) -> std::io::Result<Vec<u8>> {
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
        exchange_payload(config.socket_path.clone(), payload, timeout).await
    }
}

async fn probe_kitty_graphics(config: &HerdrConfig) -> bool {
    let request = json_rpc_request("pane.graphics.info", json!({ "pane_id": config.pane_id }));
    let Ok(mut payload) = serde_json::to_vec(&request) else {
        return false;
    };
    payload.push(b'\n');
    let Ok(response) =
        exchange_payload(config.socket_path.clone(), payload, GRAPHICS_PROBE_TIMEOUT).await
    else {
        return false;
    };
    graphics_info_reports_paintable(&response)
}

pub(crate) fn graphics_info_reports_paintable(response: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(response) else {
        return false;
    };
    value.get("error").is_none() && value.get("result").is_some()
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
        "id": format!("{SOURCE}:{}", request_id_suffix()),
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
async fn exchange_payload(
    socket_path: PathBuf,
    payload: Vec<u8>,
    timeout: Duration,
) -> std::io::Result<Vec<u8>> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    tokio::time::timeout(timeout, async move {
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
async fn exchange_payload(
    _socket_path: PathBuf,
    _payload: Vec<u8>,
    _timeout: Duration,
) -> std::io::Result<Vec<u8>> {
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
            Self::bind_with_response(socket_path, b"{}\n").await
        }

        pub(crate) async fn bind_with_response(
            socket_path: &Path,
            response: &'static [u8],
        ) -> Self {
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
                        stream.get_mut().write_all(response).await.unwrap();
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    });
                }
            });
            Self { requests }
        }

        pub(crate) async fn bind_with_responses(
            socket_path: &Path,
            responses: Vec<&'static [u8]>,
        ) -> Self {
            use std::{collections::VecDeque, sync::Arc};

            let listener = tokio::net::UnixListener::bind(socket_path).unwrap();
            let (tx, requests) = tokio::sync::mpsc::unbounded_channel();
            let responses = Arc::new(std::sync::Mutex::new(VecDeque::from(responses)));
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let tx = tx.clone();
                    let response = responses.lock().unwrap().pop_front().unwrap_or(b"{}\n");
                    tokio::spawn(async move {
                        let mut stream = BufReader::new(stream);
                        let mut line = String::new();
                        stream.read_line(&mut line).await.unwrap();
                        let request = serde_json::from_str(&line).unwrap();
                        tx.send(request).unwrap();
                        // Keep the connection open after the framed response so
                        // clients must read a newline rather than waiting for EOF.
                        stream.get_mut().write_all(response).await.unwrap();
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
