use serde_json::json;
use std::{
    env,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const REQUEST_TIMEOUT: Duration = Duration::from_millis(500);
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

        self.send(json_rpc_request("pane.report_agent", params))
            .await;
    }

    pub async fn report_session(&self, session_id: Option<&str>) {
        let (Some(config), Some(session_id)) = (&self.config, session_id) else {
            return;
        };

        self.send(json_rpc_request(
            "pane.report_agent_session",
            json!({
                "pane_id": config.pane_id,
                "source": SOURCE,
                "agent": AGENT,
                "agent_session_id": session_id,
            }),
        ))
        .await;
    }

    pub async fn release(&self) {
        let Some(config) = &self.config else {
            return;
        };

        self.send(json_rpc_request(
            "pane.release_agent",
            json!({
                "pane_id": config.pane_id,
                "source": SOURCE,
                "agent": AGENT,
            }),
        ))
        .await;
    }

    async fn send(&self, request: serde_json::Value) {
        let Some(config) = &self.config else {
            return;
        };
        let socket_path = config.socket_path.clone();
        let payload = match serde_json::to_vec(&request) {
            Ok(mut payload) => {
                payload.push(b'\n');
                payload
            }
            Err(_) => return,
        };

        let _ = tokio::time::timeout(REQUEST_TIMEOUT, send_payload(socket_path, payload)).await;
    }
}

#[cfg(unix)]
fn socket_is_reachable(path: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
fn socket_is_reachable(_path: &std::path::Path) -> bool {
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
async fn send_payload(socket_path: PathBuf, payload: Vec<u8>) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    let mut stream = UnixStream::connect(socket_path).await?;
    stream.write_all(&payload).await?;
    let _ = stream.shutdown().await;
    let mut buffer = [0_u8; 256];
    let _ = stream.read(&mut buffer).await;
    Ok(())
}

#[cfg(not(unix))]
async fn send_payload(_socket_path: PathBuf, _payload: Vec<u8>) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "herdr_tests.rs"]
mod tests;
