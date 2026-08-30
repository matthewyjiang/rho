//! Ollama device-key authentication for ollama.com.
//!
//! Matches the local Ollama client: an Ed25519 key at `~/.ollama/id_ed25519`
//! signs each request as `Authorization: <pubkey_b64>:<sig_b64>` with a `ts`
//! query parameter. Browser sign-in registers that public key at
//! `https://ollama.com/connect`.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose, Engine as _};
use rand::rngs::OsRng;
use signature::Signer;
use ssh_key::{Algorithm, LineEnding, PrivateKey};

const DEFAULT_PRIVATE_KEY_NAME: &str = "id_ed25519";
const DEFAULT_PUBLIC_KEY_NAME: &str = "id_ed25519.pub";
const OPENSSH_PEM_BEGIN: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";
const OPENSSH_PEM_END: &str = "-----END OPENSSH PRIVATE KEY-----";
const CONNECT_BASE: &str = "https://ollama.com/connect";

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
        let private_key = parse_openssh_private_key(pem)?;
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
        let private_key_openssh = private_key
            .to_openssh(LineEnding::LF)
            .map_err(|error| OllamaDeviceError::InvalidKey(error.to_string()))?
            .trim()
            .to_string();
        Ok(Self {
            private_key_openssh,
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
    /// # Next major
    ///
    /// NEXT_MAJOR(rho-providers): remove OllamaDeviceError::Browser; browser launch lives in the login dispatch layer
    #[error("could not open a browser for Ollama device login")]
    Browser,
}

#[derive(Clone, Debug)]
pub struct OllamaDeviceLogin {
    pub connect_url: String,
}

/// Starts sign-in for the local Ollama device key.
///
/// # Next major
///
/// NEXT_MAJOR(rho-providers): remove the `open_browser` argument; the dispatch layer owns browser launch
///
/// When `open_browser` is true, opens the Ollama connect page. Callers in
/// headless environments can pass false and show [`OllamaDeviceLogin::connect_url`]
/// instead. Ollama does not send a completion callback to the client.
pub async fn start_ollama_device_login(
    open_browser: bool,
) -> Result<OllamaDeviceLogin, OllamaDeviceError> {
    let key = OllamaDeviceKey::load_or_create_default()?;
    let connect_url = key.connect_url()?;
    if open_browser {
        webbrowser::open(&connect_url).map_err(|_| OllamaDeviceError::Browser)?;
    }
    Ok(OllamaDeviceLogin { connect_url })
}

/// True when a usable local Ollama device key exists on disk.
pub fn ollama_device_credentials_available() -> bool {
    OllamaDeviceKey::load_default().is_ok()
}

fn parse_openssh_private_key(pem: &str) -> Result<PrivateKey, OllamaDeviceError> {
    let mut lines = pem.trim().lines();
    if lines.next() != Some(OPENSSH_PEM_BEGIN) || lines.next_back() != Some(OPENSSH_PEM_END) {
        return Err(OllamaDeviceError::InvalidKey(
            "expected an OpenSSH private key PEM".into(),
        ));
    }

    // Ollama's Go PEM encoder wraps at 64 columns, while `ssh-key`'s strict
    // PEM decoder expects its own 70-column wrapping. Decode the valid PEM
    // payload without tying device-key support to either wrapping choice.
    let payload = lines.collect::<String>();
    let bytes = general_purpose::STANDARD
        .decode(payload)
        .map_err(|error| OllamaDeviceError::InvalidKey(error.to_string()))?;
    PrivateKey::from_bytes(&bytes).map_err(|error| OllamaDeviceError::InvalidKey(error.to_string()))
}

thread_local! {
    static TEST_KEY_DIR: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `f` with the device key directory overridden, so tests do not read the
/// developer's real `~/.ollama` key.
#[doc(hidden)]
pub fn with_ollama_device_key_dir_for_tests<T>(path: PathBuf, f: impl FnOnce() -> T) -> T {
    TEST_KEY_DIR.with(|key_dir| {
        let previous = key_dir.replace(Some(path));
        // Restore the prior value even when `f` unwinds.
        struct Restore<'a>(&'a std::cell::RefCell<Option<PathBuf>>, Option<PathBuf>);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.0.replace(self.1.take());
            }
        }
        let _guard = Restore(key_dir, previous);
        f()
    })
}

fn default_key_dir() -> Result<PathBuf, OllamaDeviceError> {
    if let Some(path) = TEST_KEY_DIR.with(|key_dir| key_dir.borrow().clone()) {
        return Ok(path);
    }
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
