//! Ollama device-key authentication for ollama.com.
//!
//! Matches the local Ollama client: an Ed25519 key at `~/.ollama/id_ed25519`
//! signs each request as `Authorization: <pubkey_b64>:<sig_b64>` with a `ts`
//! query parameter. Browser sign-in registers that public key at
//! `https://ollama.com/connect`.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose, Engine as _};
use rand::rngs::OsRng;
use serde::Deserialize;
use signature::Signer;
use ssh_key::{Algorithm, LineEnding, PrivateKey};
use tokio::time::sleep;

const DEFAULT_PRIVATE_KEY_NAME: &str = "id_ed25519";
const DEFAULT_PUBLIC_KEY_NAME: &str = "id_ed25519.pub";
const CONNECT_BASE: &str = "https://ollama.com/connect";
const WHOAMI_URL: &str = "https://ollama.com/api/me";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);
const DEFAULT_POLL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Owned Ed25519 material used to sign Ollama Cloud requests.
#[derive(Clone)]
pub struct OllamaDeviceKey {
    private_key_openssh: String,
    public_key_openssh: String,
}

impl std::fmt::Debug for OllamaDeviceKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OllamaDeviceKey")
            .field("public_key_openssh", &self.public_key_openssh)
            .field("private_key_openssh", &"[REDACTED]")
            .finish()
    }
}

impl OllamaDeviceKey {
    /// Loads the device key from the default Ollama path, generating one if missing.
    pub fn load_or_create_default() -> Result<Self, OllamaDeviceError> {
        Self::load_or_create(&default_key_dir()?)
    }

    /// Loads an existing device key from the default Ollama path.
    pub fn load_default() -> Result<Self, OllamaDeviceError> {
        Self::load_from_dir(&default_key_dir()?)
    }

    pub fn load_or_create(dir: &Path) -> Result<Self, OllamaDeviceError> {
        match Self::load_from_dir(dir) {
            Ok(key) => Ok(key),
            Err(OllamaDeviceError::MissingKey(_)) => {
                ensure_key_dir(dir)?;
                match create_key_pair(dir) {
                    Ok(key) => Ok(key),
                    // Another process won the create race; reload its key.
                    Err(OllamaDeviceError::AlreadyExists) => Self::load_from_dir(dir),
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    pub fn load_from_dir(dir: &Path) -> Result<Self, OllamaDeviceError> {
        let private_path = dir.join(DEFAULT_PRIVATE_KEY_NAME);
        if !private_path.exists() {
            return Err(OllamaDeviceError::MissingKey(private_path));
        }
        let pem = fs::read_to_string(&private_path).map_err(|error| {
            OllamaDeviceError::Io(format!("read {}: {error}", private_path.display()))
        })?;
        Self::from_openssh_private_key(&pem)
    }

    pub fn from_openssh_private_key(pem: &str) -> Result<Self, OllamaDeviceError> {
        let private_key = PrivateKey::from_openssh(pem)
            .map_err(|error| OllamaDeviceError::InvalidKey(error.to_string()))?;
        if private_key.is_encrypted() {
            return Err(OllamaDeviceError::InvalidKey(
                "encrypted Ollama device keys are not supported".into(),
            ));
        }
        if !private_key.algorithm().is_ed25519() {
            return Err(OllamaDeviceError::InvalidKey(format!(
                "expected Ed25519 device key, found {}",
                private_key.algorithm()
            )));
        }
        let public_key_openssh = private_key
            .public_key()
            .to_openssh()
            .map_err(|error| OllamaDeviceError::InvalidKey(error.to_string()))?
            .trim()
            .to_string();
        Ok(Self {
            private_key_openssh: pem.trim().to_string(),
            public_key_openssh,
        })
    }

    pub fn public_key_openssh(&self) -> &str {
        &self.public_key_openssh
    }

    /// Builds the browser connect URL used by `ollama signin`.
    pub fn connect_url(&self) -> Result<String, OllamaDeviceError> {
        let encoded_key =
            general_purpose::URL_SAFE_NO_PAD.encode(self.public_key_openssh.as_bytes());
        let mut url = url::Url::parse(CONNECT_BASE)
            .map_err(|error| OllamaDeviceError::InvalidKey(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("name", &device_name())
            .append_pair("key", &encoded_key);
        Ok(url.into())
    }

    /// Signs an Ollama request challenge as `pubkey_b64:signature_b64`.
    pub fn sign_authorization(&self, challenge: &str) -> Result<String, OllamaDeviceError> {
        let private_key = PrivateKey::from_openssh(&self.private_key_openssh)
            .map_err(|error| OllamaDeviceError::InvalidKey(error.to_string()))?;
        let signature: ssh_key::Signature = private_key
            .try_sign(challenge.as_bytes())
            .map_err(|error| OllamaDeviceError::Sign(error.to_string()))?;
        let pubkey_b64 = public_key_payload_b64(&self.public_key_openssh)?;
        let sig_b64 = general_purpose::STANDARD.encode(signature.as_bytes());
        Ok(format!("{pubkey_b64}:{sig_b64}"))
    }

    /// Applies device-key auth headers/query params to a request builder.
    pub fn authorize_request(
        &self,
        method: &str,
        url: url::Url,
    ) -> Result<(url::Url, String), OllamaDeviceError> {
        let ts = unix_timestamp_secs()?.to_string();
        let mut url = url;
        url.query_pairs_mut().append_pair("ts", &ts);
        let path_and_query = match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_string(),
        };
        let challenge = format!("{method},{path_and_query}");
        let authorization = self.sign_authorization(&challenge)?;
        Ok((url, authorization))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OllamaDeviceError {
    #[error("missing Ollama device key at {0}")]
    MissingKey(PathBuf),
    #[error("invalid Ollama device key: {0}")]
    InvalidKey(String),
    #[error("could not sign Ollama device challenge: {0}")]
    Sign(String),
    #[error("Ollama device key I/O error: {0}")]
    Io(String),
    #[error("Ollama device login setup failed: {0}")]
    Setup(String),
    #[error("Ollama device key already exists")]
    AlreadyExists,
    #[error("could not open a browser for Ollama device login")]
    Browser,
    #[error("timed out waiting for Ollama device login")]
    Timeout,
    #[error("Ollama device login request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Ollama device login failed: {0}")]
    Denied(String),
}

#[derive(Clone)]
pub struct OllamaDeviceLogin {
    pub connect_url: String,
    pub key: OllamaDeviceKey,
    /// True when the key was already registered before this login attempt.
    pub already_registered: bool,
    expires_in: Duration,
    interval: Duration,
}

impl std::fmt::Debug for OllamaDeviceLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OllamaDeviceLogin")
            .field("connect_url", &self.connect_url)
            .field("key", &self.key)
            .field("already_registered", &self.already_registered)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

/// Starts sign-in for the local Ollama device key.
///
/// When `open_browser` is true and the key is not already registered, opens the
/// Ollama connect page. Callers in headless environments can pass false and show
/// [`OllamaDeviceLogin::connect_url`] instead.
pub async fn start_ollama_device_login(
    open_browser: bool,
) -> Result<OllamaDeviceLogin, OllamaDeviceError> {
    let key = OllamaDeviceKey::load_or_create_default()?;
    let connect_url = key.connect_url()?;
    // Already registered keys finish immediately without forcing a browser hop.
    if whoami(&http_client()?, &key).await?.is_some() {
        return Ok(OllamaDeviceLogin {
            connect_url,
            key,
            already_registered: true,
            expires_in: Duration::from_secs(1),
            interval: Duration::from_millis(1),
        });
    }
    if open_browser {
        webbrowser::open(&connect_url).map_err(|_| OllamaDeviceError::Browser)?;
    }
    Ok(OllamaDeviceLogin {
        connect_url,
        key,
        already_registered: false,
        expires_in: DEFAULT_POLL_TIMEOUT,
        interval: DEFAULT_POLL_INTERVAL,
    })
}

/// Polls ollama.com until the device key is associated with an account.
///
/// The local key is the credential; nothing is persisted in Rho's store.
pub async fn complete_ollama_device_login(
    login: OllamaDeviceLogin,
) -> Result<(), OllamaDeviceError> {
    let client = http_client()?;
    let deadline = Instant::now() + login.expires_in;
    loop {
        if let Some(username) = whoami(&client, &login.key).await? {
            let _ = username;
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(OllamaDeviceError::Timeout);
        }
        sleep(login.interval).await;
    }
}

/// True when a usable local Ollama device key exists on disk.
pub fn ollama_device_credentials_available() -> bool {
    OllamaDeviceKey::load_default().is_ok()
}

async fn whoami(
    client: &reqwest::Client,
    key: &OllamaDeviceKey,
) -> Result<Option<String>, OllamaDeviceError> {
    let url =
        url::Url::parse(WHOAMI_URL).map_err(|error| OllamaDeviceError::Setup(error.to_string()))?;
    let (url, authorization) = key.authorize_request(reqwest::Method::POST.as_str(), url)?;
    let response = client
        .post(url)
        .header(reqwest::header::AUTHORIZATION, authorization)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::USER_AGENT, crate::rho_user_agent())
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Ok(None);
    }
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(OllamaDeviceError::Denied(format!(
            "HTTP {status}: {}",
            body.trim()
        )));
    }
    #[derive(Deserialize)]
    struct UserResponse {
        name: Option<String>,
    }
    let user = response.json::<UserResponse>().await?;
    let name = user
        .name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());
    Ok(name)
}

fn http_client() -> Result<reqwest::Client, OllamaDeviceError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(OllamaDeviceError::Request)
}

fn default_key_dir() -> Result<PathBuf, OllamaDeviceError> {
    if let Ok(path) = std::env::var("OLLAMA_DEVICE_KEY_DIR") {
        let path = path.trim();
        if !path.is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    let home = dirs_home().ok_or_else(|| {
        OllamaDeviceError::Io("could not determine home directory for ~/.ollama".into())
    })?;
    Ok(home.join(".ollama"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn ensure_key_dir(dir: &Path) -> Result<(), OllamaDeviceError> {
    fs::create_dir_all(dir)
        .map_err(|error| OllamaDeviceError::Io(format!("create {}: {error}", dir.display())))
}

fn generate_key() -> Result<OllamaDeviceKey, OllamaDeviceError> {
    let private_key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
        .map_err(|error| OllamaDeviceError::Setup(error.to_string()))?;
    let pem = private_key
        .to_openssh(LineEnding::LF)
        .map_err(|error| OllamaDeviceError::Setup(error.to_string()))?
        .to_string();
    OllamaDeviceKey::from_openssh_private_key(&pem)
}

/// Creates a new key pair without truncating an existing private key.
fn create_key_pair(dir: &Path) -> Result<OllamaDeviceKey, OllamaDeviceError> {
    let key = generate_key()?;
    let private_path = dir.join(DEFAULT_PRIVATE_KEY_NAME);
    let public_path = dir.join(DEFAULT_PUBLIC_KEY_NAME);
    write_private_key_exclusive(
        &private_path,
        format!("{}\n", key.private_key_openssh.trim()),
    )?;
    // Public key is derived from private; rewrite is safe if the private write won.
    fs::write(&public_path, format!("{}\n", key.public_key_openssh.trim())).map_err(|error| {
        OllamaDeviceError::Io(format!("write {}: {error}", public_path.display()))
    })?;
    Ok(key)
}

#[cfg(unix)]
fn write_private_key_exclusive(path: &Path, contents: String) -> Result<(), OllamaDeviceError> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};

    // create_new refuses to truncate an existing key if two processes race.
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(mut file) => file
            .write_all(contents.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|error| OllamaDeviceError::Io(format!("write {}: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(OllamaDeviceError::AlreadyExists)
        }
        Err(error) => Err(OllamaDeviceError::Io(format!(
            "create {}: {error}",
            path.display()
        ))),
    }
}

#[cfg(not(unix))]
fn write_private_key_exclusive(path: &Path, contents: String) -> Result<(), OllamaDeviceError> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(contents.as_bytes())
        }) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(OllamaDeviceError::AlreadyExists)
        }
        Err(error) => Err(OllamaDeviceError::Io(format!(
            "write {}: {error}",
            path.display()
        ))),
    }
}

fn public_key_payload_b64(openssh: &str) -> Result<String, OllamaDeviceError> {
    let mut parts = openssh.split_whitespace();
    let algorithm = parts.next().unwrap_or_default();
    let payload = parts
        .next()
        .ok_or_else(|| OllamaDeviceError::InvalidKey("malformed OpenSSH public key".into()))?;
    if algorithm != "ssh-ed25519" {
        return Err(OllamaDeviceError::InvalidKey(format!(
            "expected ssh-ed25519 public key, found {algorithm}"
        )));
    }
    Ok(payload.to_string())
}

fn device_name() -> String {
    let name = gethostname::gethostname();
    let name = name.to_string_lossy();
    let name = name.trim();
    if name.is_empty() {
        "rho".into()
    } else {
        name.to_string()
    }
}

fn unix_timestamp_secs() -> Result<u64, OllamaDeviceError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| OllamaDeviceError::Setup(format!("system clock error: {error}")))
}

#[cfg(test)]
#[path = "ollama_device_tests.rs"]
mod tests;
